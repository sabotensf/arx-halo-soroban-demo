extern crate std;

use p256::ecdsa::{signature::Signer, Signature, SigningKey};
use soroban_sdk::{
    testutils::{Address as _, AuthorizedFunction},
    token::{StellarAssetClient, TokenClient},
    Address, Bytes, BytesN, Env, Error, IntoVal, String, Symbol,
};

use crate::{CardError, RecordPoolCard, RecordPoolCardClient};

const USDC: i128 = 10_000_000; // 7 decimals, like the USDC Stellar asset
const CARD_PRICE: i128 = 25 * USDC;
const ROYALTY_BPS: u32 = 1_000; // 10%

/// A simulated card secure element.
struct Chip {
    key: SigningKey,
}

impl Chip {
    fn new(seed: u8) -> Self {
        let mut bytes = [0u8; 32];
        bytes[31] = seed;
        bytes[0] = 0x42;
        Self {
            key: SigningKey::from_slice(&bytes).unwrap(),
        }
    }

    fn pubkey(&self, e: &Env) -> BytesN<65> {
        let point = self.key.verifying_key().to_encoded_point(false);
        BytesN::from_array(e, &point.as_bytes().try_into().unwrap())
    }

    fn sign(&self, e: &Env, challenge: &[u8]) -> BytesN<64> {
        let sig: Signature = self.key.sign(challenge);
        let sig = sig.normalize_s().unwrap_or(sig);
        BytesN::from_array(e, &sig.to_bytes().into())
    }
}

struct Setup<'a> {
    e: Env,
    admin: Address,
    treasury: Address,
    card: RecordPoolCardClient<'a>,
    usdc: TokenClient<'a>,
    usdc_admin: StellarAssetClient<'a>,
}

fn setup<'a>() -> Setup<'a> {
    let e = Env::default();
    e.mock_all_auths();

    let admin = Address::generate(&e);
    let treasury = Address::generate(&e);

    let sac = e.register_stellar_asset_contract_v2(Address::generate(&e));
    let usdc = TokenClient::new(&e, &sac.address());
    let usdc_admin = StellarAssetClient::new(&e, &sac.address());

    let contract_id = e.register(
        RecordPoolCard,
        (
            admin.clone(),
            sac.address(),
            treasury.clone(),
            ROYALTY_BPS,
            String::from_str(&e, "https://cards.recordpool.io/meta/"),
            String::from_str(&e, "RecordPool Card"),
            String::from_str(&e, "RPCARD"),
        ),
    );
    let card = RecordPoolCardClient::new(&e, &contract_id);

    Setup {
        e,
        admin,
        treasury,
        card,
        usdc,
        usdc_admin,
    }
}

fn funded(s: &Setup, amount: i128) -> Address {
    let who = Address::generate(&s.e);
    s.usdc_admin.mint(&who, &amount);
    who
}

fn contract_err(err: CardError) -> Error {
    Error::from_contract_error(err as u32)
}

/// Registers a chip and has `buyer` purchase it. Returns token id.
fn register_and_buy(s: &Setup, chip: &Chip, buyer: &Address) -> u32 {
    let chip_id = s
        .card
        .register_card(&chip.pubkey(&s.e), &CARD_PRICE, &s.admin);
    s.card.purchase(buyer, &chip_id)
}

// ---------------------------------------------------------------- metadata

#[test]
fn metadata_and_roles() {
    let s = setup();
    assert_eq!(s.card.name(), String::from_str(&s.e, "RecordPool Card"));
    assert_eq!(s.card.symbol(), String::from_str(&s.e, "RPCARD"));
    assert_eq!(s.card.get_admin(), Some(s.admin.clone()));
    assert!(s
        .card
        .has_role(&s.admin, &Symbol::new(&s.e, "issuer"))
        .is_some());
}

// ------------------------------------------------------------ primary sale

#[test]
fn purchase_pays_treasury_and_mints_bound_token() {
    let s = setup();
    let chip = Chip::new(1);
    let buyer = funded(&s, 100 * USDC);

    let chip_id = s
        .card
        .register_card(&chip.pubkey(&s.e), &CARD_PRICE, &s.admin);
    assert_eq!(s.card.card_price(&chip_id), Some(CARD_PRICE));

    let token_id = s.card.purchase(&buyer, &chip_id);

    // Buyer's signature covered both the purchase and the USDC transfer.
    let auths = s.e.auths();
    assert_eq!(auths.len(), 1);
    assert_eq!(auths[0].0, buyer);
    assert_eq!(
        auths[0].1.function,
        AuthorizedFunction::Contract((
            s.card.address.clone(),
            Symbol::new(&s.e, "purchase"),
            (buyer.clone(), chip_id.clone()).into_val(&s.e),
        ))
    );

    assert_eq!(s.usdc.balance(&buyer), 75 * USDC);
    assert_eq!(s.usdc.balance(&s.treasury), CARD_PRICE);
    assert_eq!(s.card.owner_of(&token_id), buyer);
    assert_eq!(s.card.balance(&buyer), 1);
    assert_eq!(s.card.total_supply(), 1);
    assert_eq!(s.card.get_owner_token_id(&buyer, &0), token_id);
    assert_eq!(s.card.token_for_chip(&chip_id), Some(token_id));
    assert_eq!(s.card.chip_pubkey(&token_id), chip.pubkey(&s.e));
    assert_eq!(s.card.card_price(&chip_id), None);
    assert_eq!(
        s.card.token_uri(&token_id),
        String::from_str(&s.e, "https://cards.recordpool.io/meta/0")
    );
}

#[test]
fn only_issuer_can_register() {
    let s = setup();
    let rando = Address::generate(&s.e);
    let res = s
        .card
        .try_register_card(&Chip::new(1).pubkey(&s.e), &CARD_PRICE, &rando);
    assert!(res.is_err());
}

#[test]
fn granted_issuer_can_register() {
    let s = setup();
    let factory = Address::generate(&s.e);
    s.card
        .grant_role(&factory, &Symbol::new(&s.e, "issuer"), &s.admin);
    s.card
        .register_card(&Chip::new(1).pubkey(&s.e), &CARD_PRICE, &factory);
}

#[test]
fn cannot_register_same_chip_twice() {
    let s = setup();
    let pk = Chip::new(1).pubkey(&s.e);
    s.card.register_card(&pk, &CARD_PRICE, &s.admin);
    assert_eq!(
        s.card.try_register_card(&pk, &CARD_PRICE, &s.admin),
        Err(Ok(contract_err(CardError::CardAlreadyRegistered)))
    );
}

#[test]
fn cannot_reregister_after_mint() {
    let s = setup();
    let chip = Chip::new(1);
    register_and_buy(&s, &chip, &funded(&s, CARD_PRICE));
    assert_eq!(
        s.card
            .try_register_card(&chip.pubkey(&s.e), &CARD_PRICE, &s.admin),
        Err(Ok(contract_err(CardError::CardAlreadyRegistered)))
    );
}

#[test]
fn rejects_non_positive_price() {
    let s = setup();
    assert_eq!(
        s.card
            .try_register_card(&Chip::new(1).pubkey(&s.e), &0, &s.admin),
        Err(Ok(contract_err(CardError::InvalidPrice)))
    );
}

#[test]
fn purchase_unknown_chip_fails() {
    let s = setup();
    let buyer = funded(&s, CARD_PRICE);
    let bogus = BytesN::from_array(&s.e, &[7u8; 32]);
    assert_eq!(
        s.card.try_purchase(&buyer, &bogus),
        Err(Ok(contract_err(CardError::CardNotRegistered)))
    );
}

#[test]
fn cannot_buy_same_card_twice() {
    let s = setup();
    let chip = Chip::new(1);
    let chip_id = s
        .card
        .register_card(&chip.pubkey(&s.e), &CARD_PRICE, &s.admin);
    s.card.purchase(&funded(&s, CARD_PRICE), &chip_id);
    assert_eq!(
        s.card.try_purchase(&funded(&s, CARD_PRICE), &chip_id),
        Err(Ok(contract_err(CardError::CardAlreadyMinted)))
    );
}

#[test]
fn underfunded_purchase_is_atomic() {
    let s = setup();
    let chip = Chip::new(1);
    let buyer = funded(&s, CARD_PRICE - 1);
    let chip_id = s
        .card
        .register_card(&chip.pubkey(&s.e), &CARD_PRICE, &s.admin);

    assert!(s.card.try_purchase(&buyer, &chip_id).is_err());

    // Nothing moved, card still for sale.
    assert_eq!(s.usdc.balance(&buyer), CARD_PRICE - 1);
    assert_eq!(s.card.total_supply(), 0);
    assert_eq!(s.card.card_price(&chip_id), Some(CARD_PRICE));
}

// ----------------------------------------------------------- authenticity

#[test]
fn authenticate_genuine_card_returns_owner() {
    let s = setup();
    let chip = Chip::new(1);
    let buyer = funded(&s, CARD_PRICE);
    let token_id = register_and_buy(&s, &chip, &buyer);

    let challenge = b"verifier-nonce-2026-11-08T15:00Z";
    let sig = chip.sign(&s.e, challenge);
    let owner = s
        .card
        .authenticate(&token_id, &Bytes::from_slice(&s.e, challenge), &sig);
    assert_eq!(owner, buyer);
}

#[test]
fn authenticate_rejects_counterfeit_chip() {
    let s = setup();
    let genuine = Chip::new(1);
    let counterfeit = Chip::new(2);
    let token_id = register_and_buy(&s, &genuine, &funded(&s, CARD_PRICE));

    let challenge = b"verifier-nonce";
    let sig = counterfeit.sign(&s.e, challenge);
    assert!(s
        .card
        .try_authenticate(&token_id, &Bytes::from_slice(&s.e, challenge), &sig)
        .is_err());
}

#[test]
fn authenticate_rejects_replayed_signature_on_new_challenge() {
    let s = setup();
    let chip = Chip::new(1);
    let token_id = register_and_buy(&s, &chip, &funded(&s, CARD_PRICE));

    let old_sig = chip.sign(&s.e, b"yesterday's challenge");
    assert!(s
        .card
        .try_authenticate(
            &token_id,
            &Bytes::from_slice(&s.e, b"today's challenge"),
            &old_sig
        )
        .is_err());
}

// -------------------------------------------------------- secondary market

#[test]
fn resale_enforces_default_royalty() {
    let s = setup();
    let alice = funded(&s, CARD_PRICE);
    let bob = funded(&s, 100 * USDC);
    let token_id = register_and_buy(&s, &Chip::new(1), &alice);
    let treasury_after_primary = s.usdc.balance(&s.treasury);

    s.card.list(&alice, &token_id, &(80 * USDC));
    s.card.buy(&bob, &token_id);

    // 10% of 80 = 8 USDC royalty to treasury, 72 to Alice.
    assert_eq!(
        s.usdc.balance(&s.treasury) - treasury_after_primary,
        8 * USDC
    );
    assert_eq!(s.usdc.balance(&alice), 72 * USDC);
    assert_eq!(s.usdc.balance(&bob), 20 * USDC);

    assert_eq!(s.card.owner_of(&token_id), bob);
    assert_eq!(s.card.balance(&alice), 0);
    assert_eq!(s.card.balance(&bob), 1);
    assert_eq!(s.card.get_owner_token_id(&bob, &0), token_id);
    assert_eq!(s.card.get_listing(&token_id), None);
}

#[test]
fn resale_uses_token_specific_royalty() {
    let s = setup();
    let alice = funded(&s, CARD_PRICE);
    let bob = funded(&s, 100 * USDC);
    let artist = Address::generate(&s.e);
    let token_id = register_and_buy(&s, &Chip::new(1), &alice);

    // Limited-edition card: 5% to the featured artist instead.
    s.card.set_token_royalty(&token_id, &artist, &500, &s.admin);
    s.card.list(&alice, &token_id, &(40 * USDC));
    s.card.buy(&bob, &token_id);

    assert_eq!(s.usdc.balance(&artist), 2 * USDC);
    assert_eq!(s.usdc.balance(&alice), 38 * USDC);
}

#[test]
fn only_owner_can_list() {
    let s = setup();
    let alice = funded(&s, CARD_PRICE);
    let mallory = Address::generate(&s.e);
    let token_id = register_and_buy(&s, &Chip::new(1), &alice);
    assert_eq!(
        s.card.try_list(&mallory, &token_id, &USDC),
        Err(Ok(contract_err(CardError::NotTokenOwner)))
    );
}

#[test]
fn transfer_invalidates_listing() {
    let s = setup();
    let alice = funded(&s, CARD_PRICE);
    let carol = Address::generate(&s.e);
    let bob = funded(&s, 100 * USDC);
    let token_id = register_and_buy(&s, &Chip::new(1), &alice);

    s.card.list(&alice, &token_id, &(10 * USDC));
    s.card.transfer(&alice, &carol, &token_id);
    assert_eq!(s.card.get_listing(&token_id), None);

    assert_eq!(
        s.card.try_buy(&bob, &token_id),
        Err(Ok(contract_err(CardError::NotListed)))
    );
    assert_eq!(s.card.owner_of(&token_id), carol);
    assert_eq!(s.usdc.balance(&bob), 100 * USDC);
}

#[test]
fn cancel_listing() {
    let s = setup();
    let alice = funded(&s, CARD_PRICE);
    let bob = funded(&s, 100 * USDC);
    let token_id = register_and_buy(&s, &Chip::new(1), &alice);

    s.card.list(&alice, &token_id, &(10 * USDC));
    s.card.cancel_listing(&alice, &token_id);
    assert_eq!(
        s.card.try_buy(&bob, &token_id),
        Err(Ok(contract_err(CardError::NotListed)))
    );
}

#[test]
fn seller_cannot_buy_own_listing() {
    let s = setup();
    let alice = funded(&s, CARD_PRICE + 10 * USDC);
    let token_id = register_and_buy(&s, &Chip::new(1), &alice);
    s.card.list(&alice, &token_id, &(10 * USDC));
    assert_eq!(
        s.card.try_buy(&alice, &token_id),
        Err(Ok(contract_err(CardError::SelfPurchase)))
    );
}

#[test]
fn authenticate_follows_ownership_after_resale() {
    let s = setup();
    let chip = Chip::new(1);
    let alice = funded(&s, CARD_PRICE);
    let bob = funded(&s, 100 * USDC);
    let token_id = register_and_buy(&s, &chip, &alice);

    s.card.list(&alice, &token_id, &(30 * USDC));
    s.card.buy(&bob, &token_id);

    let challenge = b"post-resale check";
    let owner = s.card.authenticate(
        &token_id,
        &Bytes::from_slice(&s.e, challenge),
        &chip.sign(&s.e, challenge),
    );
    assert_eq!(owner, bob);
}

#[test]
fn enumerates_multiple_cards() {
    let s = setup();
    let alice = funded(&s, 3 * CARD_PRICE);
    let ids: std::vec::Vec<u32> = (1..=3)
        .map(|i| register_and_buy(&s, &Chip::new(i), &alice))
        .collect();
    assert_eq!(s.card.total_supply(), 3);
    assert_eq!(s.card.balance(&alice), 3);
    for (i, id) in ids.iter().enumerate() {
        assert_eq!(s.card.get_owner_token_id(&alice, &(i as u32)), *id);
    }
}
