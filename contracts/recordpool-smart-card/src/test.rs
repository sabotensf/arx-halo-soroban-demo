extern crate std;

use k256::ecdsa::SigningKey;
use sha2::{Digest, Sha256};
use soroban_sdk::{
    testutils::{
        storage::{Instance as _, Persistent as _},
        Address as _, AuthorizedFunction, Events as _, Ledger,
    },
    token::{StellarAssetClient, TokenClient},
    Address, Bytes, BytesN, Env, Error, Event, IntoVal, String, Symbol,
};
use stellar_tokens::non_fungible::{Mint, Transfer};

use crate::{
    contract::{SEP53_PREFIX, TRANSFER_TAG, VERIFY_TAG},
    events::{CardPurchased, CardRegistered, ChipSet, NonceRotated},
    storage::{CardKey, INSTANCE_EXTEND_AMOUNT, INSTANCE_TTL_THRESHOLD, PERSISTENT_EXTEND_AMOUNT},
    CardError, RecordPoolSmartCard, RecordPoolSmartCardClient,
};

const USDC: i128 = 10_000_000; // 7 decimals, like the USDC Stellar asset
const CARD_PRICE: i128 = 25 * USDC;
const WINDOW: u64 = 300; // seconds a chip signature stays valid
const NOW: u64 = 1_800_000_000;

/// A simulated HaLo chip, key slot 1 (secp256k1).
struct Chip {
    key: SigningKey,
}

/// A chip signature as the contract takes it: r || s and a recovery id.
#[derive(Clone)]
struct Sig {
    rs: BytesN<64>,
    recovery_id: u32,
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

    /// SHA-256 of the uncompressed public key (`publicKeys["1"]`).
    fn id(&self, e: &Env) -> BytesN<32> {
        let point = self.key.verifying_key().to_encoded_point(false);
        BytesN::from_array(e, &Sha256::digest(point.as_bytes()).into())
    }

    /// The SEP-53 digest of `message`, signed as HaLo `sign { keyNo: 1, digest }`
    /// does (`signature.raw` r, s and v − 27).
    fn sign(&self, e: &Env, message: &[u8]) -> Sig {
        let digest = Sha256::new()
            .chain_update(SEP53_PREFIX)
            .chain_update(message)
            .finalize();
        let (sig, rid) = self.key.sign_prehash_recoverable(&digest).unwrap();
        Sig {
            rs: BytesN::from_array(e, &sig.to_bytes().into()),
            recovery_id: rid.to_byte() as u32,
        }
    }
}

struct Setup<'a> {
    e: Env,
    admin: Address,
    treasury: Address,
    card: RecordPoolSmartCardClient<'a>,
    usdc: TokenClient<'a>,
    usdc_admin: StellarAssetClient<'a>,
}

fn deploy<'a>(
    e: &Env,
    admin: &Address,
    treasury: &Address,
    usdc: &Address,
) -> RecordPoolSmartCardClient<'a> {
    let id = e.register(
        RecordPoolSmartCard,
        (
            admin.clone(),
            usdc.clone(),
            treasury.clone(),
            WINDOW,
            String::from_str(e, "http://localhost:8000/meta/"),
            String::from_str(e, "RecordPool Smart Card"),
            String::from_str(e, "RPCARD"),
        ),
    );
    RecordPoolSmartCardClient::new(e, &id)
}

fn setup<'a>() -> Setup<'a> {
    let e = Env::default();
    e.mock_all_auths();
    e.ledger().set_timestamp(NOW);

    let admin = Address::generate(&e);
    let treasury = Address::generate(&e);
    let sac = e.register_stellar_asset_contract_v2(Address::generate(&e));
    let card = deploy(&e, &admin, &treasury, &sac.address());

    Setup {
        usdc: TokenClient::new(&e, &sac.address()),
        usdc_admin: StellarAssetClient::new(&e, &sac.address()),
        e,
        admin,
        treasury,
        card,
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

/// OpenZeppelin AccessControl's `Unauthorized`.
const ACCESS_UNAUTHORIZED: u32 = 2000;

fn to_vec(b: &Bytes) -> std::vec::Vec<u8> {
    b.iter().collect()
}

/// A "prove this card is genuine" challenge: the verify tag + `challenge`.
fn verify_data(e: &Env, challenge: &[u8]) -> Bytes {
    let mut data = Bytes::from_slice(e, VERIFY_TAG);
    data.extend_from_slice(challenge);
    data
}

const TRACK: &str = "http://localhost:8000/star-dust.mp3";

fn track(e: &Env) -> String {
    String::from_str(e, TRACK)
}

fn no_extras(e: &Env) -> Bytes {
    Bytes::new(e)
}

/// What a phone does on a tap: ask `card` for the message naming `to`, have
/// the chip sign it.
fn tap_on(card: &RecordPoolSmartCardClient, e: &Env, chip: &Chip, to: &Address, ts: u64) -> Sig {
    let msg = card.signature_message(to, &chip.id(e), &ts, &no_extras(e));
    chip.sign(e, &to_vec(&msg))
}

fn tap(s: &Setup, chip: &Chip, to: &Address) -> (Sig, u64) {
    let ts = s.e.ledger().timestamp();
    (tap_on(&s.card, &s.e, chip, to, ts), ts)
}

fn purchase(s: &Setup, buyer: &Address, chip: &Chip, sig: &Sig, ts: u64) -> Result<u32, Error> {
    s.card
        .try_purchase(
            buyer,
            &chip.id(&s.e),
            &sig.rs,
            &sig.recovery_id,
            &ts,
            &no_extras(&s.e),
        )
        .map(|r| r.unwrap().token_id)
        .map_err(|e| e.unwrap())
}

fn transfer_token(s: &Setup, to: &Address, chip: &Chip, sig: &Sig, ts: u64) -> Result<u32, Error> {
    s.card
        .try_transfer_token(
            to,
            &chip.id(&s.e),
            &sig.rs,
            &sig.recovery_id,
            &ts,
            &no_extras(&s.e),
        )
        .map(|r| r.unwrap())
        .map_err(|e| e.unwrap())
}

/// Registers a chip and has `buyer` tap it and buy it. Returns token id.
fn register_and_buy(s: &Setup, chip: &Chip, buyer: &Address) -> u32 {
    s.card
        .register_card(&chip.id(&s.e), &CARD_PRICE, &track(&s.e), &s.admin);
    let (sig, ts) = tap(s, chip, buyer);
    purchase(s, buyer, chip, &sig, ts).unwrap()
}

// ---------------------------------------------------------------- metadata

#[test]
fn metadata_and_roles() {
    let s = setup();
    assert_eq!(
        s.card.name(),
        String::from_str(&s.e, "RecordPool Smart Card")
    );
    assert_eq!(s.card.symbol(), String::from_str(&s.e, "RPCARD"));
    assert_eq!(s.card.get_admin(), Some(s.admin.clone()));
    assert!(s
        .card
        .has_role(&s.admin, &Symbol::new(&s.e, "issuer"))
        .is_some());
    assert_eq!(s.card.config().max_duration_window, WINDOW);
}

#[test]
fn sep53_prefix() {
    // "Stellar Signed Message:\n", 24 bytes (SEP-53).
    assert_eq!(SEP53_PREFIX.len(), 24);
    assert_eq!(SEP53_PREFIX, "Stellar Signed Message:\n".as_bytes());
}

// ------------------------------------------------------------ primary sale

#[test]
fn purchase_with_tap_pays_treasury_and_mints() {
    let s = setup();
    let chip = Chip::new(1);
    let buyer = funded(&s, 100 * USDC);
    let chip_id = chip.id(&s.e);

    s.card
        .register_card(&chip_id, &CARD_PRICE, &track(&s.e), &s.admin);
    assert_eq!(s.card.card_price(&chip_id), Some(CARD_PRICE));
    assert_eq!(
        s.card.chip_nonce(&chip_id),
        BytesN::from_array(&s.e, &[0; 32])
    );

    let (sig, ts) = tap(&s, &chip, &buyer);
    let bought = s.card.purchase(
        &buyer,
        &chip_id,
        &sig.rs,
        &sig.recovery_id,
        &ts,
        &no_extras(&s.e),
    );
    let token_id = bought.token_id;
    assert_eq!(bought.music_url, track(&s.e));

    // The buyer's one signature covered the purchase and the USDC transfer;
    // the chip's signature is checked by the contract, not the auth system.
    let auths = s.e.auths();
    assert_eq!(auths.len(), 1);
    assert_eq!(auths[0].0, buyer);
    assert_eq!(
        auths[0].1.function,
        AuthorizedFunction::Contract((
            s.card.address.clone(),
            Symbol::new(&s.e, "purchase"),
            (
                buyer.clone(),
                chip_id.clone(),
                sig.rs.clone(),
                sig.recovery_id,
                ts,
                no_extras(&s.e)
            )
                .into_val(&s.e),
        ))
    );

    assert_eq!(s.usdc.balance(&buyer), 75 * USDC);
    assert_eq!(s.usdc.balance(&s.treasury), CARD_PRICE);
    assert_eq!(s.card.owner_of(&token_id), buyer);
    assert_eq!(s.card.total_supply(), 1);
    assert_eq!(s.card.get_owner_token_id(&buyer, &0), token_id);
    assert_eq!(s.card.token_id_for_chip(&chip_id), token_id);
    assert_eq!(s.card.chip_id_for_token(&token_id), chip_id);
    assert_eq!(s.card.card_price(&chip_id), None);
    assert_ne!(
        s.card.chip_nonce(&chip_id),
        BytesN::from_array(&s.e, &[0; 32])
    );
    assert_eq!(
        s.card.token_uri(&token_id),
        String::from_str(&s.e, "http://localhost:8000/meta/0")
    );
}

#[test]
fn purchase_requires_the_chip() {
    let s = setup();
    let genuine = Chip::new(1);
    let counterfeit = Chip::new(2);
    let buyer = funded(&s, CARD_PRICE);
    s.card
        .register_card(&genuine.id(&s.e), &CARD_PRICE, &track(&s.e), &s.admin);

    // The right message, signed by a different chip.
    let ts = s.e.ledger().timestamp();
    let msg = s
        .card
        .signature_message(&buyer, &genuine.id(&s.e), &ts, &no_extras(&s.e));
    let fake = counterfeit.sign(&s.e, &to_vec(&msg));
    assert_eq!(
        purchase(&s, &buyer, &genuine, &fake, ts),
        Err(contract_err(CardError::InvalidSignature))
    );
    assert_eq!(s.usdc.balance(&buyer), CARD_PRICE);
}

#[test]
fn purchase_signature_names_the_buyer() {
    // A tap signed for Alice can't be used by Bob to buy the card.
    let s = setup();
    let chip = Chip::new(1);
    let alice = funded(&s, CARD_PRICE);
    let bob = funded(&s, CARD_PRICE);
    s.card
        .register_card(&chip.id(&s.e), &CARD_PRICE, &track(&s.e), &s.admin);

    let (sig, ts) = tap(&s, &chip, &alice);
    assert_eq!(
        purchase(&s, &bob, &chip, &sig, ts),
        Err(contract_err(CardError::InvalidSignature))
    );
}

#[test]
fn only_issuer_can_register() {
    let s = setup();
    let rando = Address::generate(&s.e);
    assert_eq!(
        s.card
            .try_register_card(&Chip::new(1).id(&s.e), &CARD_PRICE, &track(&s.e), &rando),
        Err(Ok(Error::from_contract_error(ACCESS_UNAUTHORIZED)))
    );

    s.card
        .grant_role(&rando, &Symbol::new(&s.e, "issuer"), &s.admin);
    s.card
        .register_card(&Chip::new(1).id(&s.e), &CARD_PRICE, &track(&s.e), &rando);
}

#[test]
fn register_validation() {
    let s = setup();
    let id = Chip::new(1).id(&s.e);
    assert_eq!(
        s.card.try_register_card(&id, &0, &track(&s.e), &s.admin),
        Err(Ok(contract_err(CardError::InvalidPrice)))
    );
    s.card
        .register_card(&id, &CARD_PRICE, &track(&s.e), &s.admin);
    assert_eq!(
        s.card
            .try_register_card(&id, &CARD_PRICE, &track(&s.e), &s.admin),
        Err(Ok(contract_err(CardError::CardAlreadyRegistered)))
    );
}

#[test]
fn purchase_unknown_or_sold_card_fails() {
    let s = setup();
    let chip = Chip::new(1);
    let buyer = funded(&s, 2 * CARD_PRICE);
    let (sig, ts) = tap(&s, &chip, &buyer);
    assert_eq!(
        purchase(&s, &buyer, &chip, &sig, ts),
        Err(contract_err(CardError::CardNotRegistered))
    );

    register_and_buy(&s, &chip, &buyer);
    let (sig, ts) = tap(&s, &chip, &buyer);
    assert_eq!(
        purchase(&s, &buyer, &chip, &sig, ts),
        Err(contract_err(CardError::CardAlreadyMinted))
    );
}

#[test]
fn underfunded_purchase_is_atomic() {
    let s = setup();
    let chip = Chip::new(1);
    let buyer = funded(&s, CARD_PRICE - 1);
    s.card
        .register_card(&chip.id(&s.e), &CARD_PRICE, &track(&s.e), &s.admin);

    let (sig, ts) = tap(&s, &chip, &buyer);
    assert!(s
        .card
        .try_purchase(
            &buyer,
            &chip.id(&s.e),
            &sig.rs,
            &sig.recovery_id,
            &ts,
            &no_extras(&s.e)
        )
        .is_err());

    // Nothing moved, card still for sale, and the chip's nonce is unchanged
    // so the same tap still works once the buyer has funds.
    assert_eq!(s.usdc.balance(&buyer), CARD_PRICE - 1);
    assert_eq!(s.card.total_supply(), 0);
    assert_eq!(s.card.card_price(&chip.id(&s.e)), Some(CARD_PRICE));
    s.usdc_admin.mint(&buyer, &1);
    purchase(&s, &buyer, &chip, &sig, ts).unwrap();
}

// ---------------------------------------------------------- tap to transfer

#[test]
fn transfer_token_moves_card_to_new_holder() {
    let s = setup();
    let chip = Chip::new(1);
    let alice = funded(&s, CARD_PRICE);
    let bob = Address::generate(&s.e);
    let token_id = register_and_buy(&s, &chip, &alice);

    // Alice hands Bob the card; Bob taps it.
    let (sig, ts) = tap(&s, &chip, &bob);
    let moved = s.card.transfer_token(
        &bob,
        &chip.id(&s.e),
        &sig.rs,
        &sig.recovery_id,
        &ts,
        &no_extras(&s.e),
    );

    assert_eq!(moved, token_id);
    assert!(s.e.auths().is_empty()); // no wallet signature involved
    assert_eq!(s.card.owner_of(&token_id), bob);
    assert_eq!(s.card.balance(&alice), 0);
    assert_eq!(s.card.balance(&bob), 1);
    assert_eq!(s.card.get_owner_token_id(&bob, &0), token_id);
}

#[test]
fn chip_signature_is_single_use() {
    let s = setup();
    let chip = Chip::new(1);
    let alice = funded(&s, CARD_PRICE);
    let bob = Address::generate(&s.e);
    register_and_buy(&s, &chip, &alice);

    let (sig, ts) = tap(&s, &chip, &bob);
    transfer_token(&s, &bob, &chip, &sig, ts).unwrap();
    // Same signature again: the nonce has moved on, so it no longer matches.
    assert_eq!(
        transfer_token(&s, &bob, &chip, &sig, ts),
        Err(contract_err(CardError::InvalidSignature))
    );
}

#[test]
fn transfer_signature_names_the_recipient() {
    let s = setup();
    let chip = Chip::new(1);
    let alice = funded(&s, CARD_PRICE);
    let bob = Address::generate(&s.e);
    let mallory = Address::generate(&s.e);
    register_and_buy(&s, &chip, &alice);

    let (sig, ts) = tap(&s, &chip, &bob);
    assert_eq!(
        transfer_token(&s, &mallory, &chip, &sig, ts),
        Err(contract_err(CardError::InvalidSignature))
    );
}

#[test]
fn extras_are_signed() {
    let s = setup();
    let chip = Chip::new(1);
    let alice = funded(&s, CARD_PRICE);
    let bob = Address::generate(&s.e);
    register_and_buy(&s, &chip, &alice);

    let extras = Bytes::from_slice(&s.e, b"gift");
    let ts = s.e.ledger().timestamp();
    let msg = s.card.signature_message(&bob, &chip.id(&s.e), &ts, &extras);
    let sig = chip.sign(&s.e, &to_vec(&msg));
    assert_eq!(
        transfer_token(&s, &bob, &chip, &sig, ts),
        Err(contract_err(CardError::InvalidSignature))
    );
    s.card.transfer_token(
        &bob,
        &chip.id(&s.e),
        &sig.rs,
        &sig.recovery_id,
        &ts,
        &extras,
    );
}

#[test]
fn signature_timestamp_window() {
    let s = setup();
    let chip = Chip::new(1);
    let alice = funded(&s, CARD_PRICE);
    let bob = Address::generate(&s.e);
    register_and_buy(&s, &chip, &alice);
    let sign_at = |ts: u64| tap_on(&s.card, &s.e, &chip, &bob, ts);

    let future = NOW + 1;
    assert_eq!(
        transfer_token(&s, &bob, &chip, &sign_at(future), future),
        Err(contract_err(CardError::SignatureTimestampInFuture))
    );

    let stale = NOW - WINDOW - 1;
    assert_eq!(
        transfer_token(&s, &bob, &chip, &sign_at(stale), stale),
        Err(contract_err(CardError::SignatureTimestampTooOld))
    );

    // Exactly at the edge of the window is still valid.
    let edge = NOW - WINDOW;
    transfer_token(&s, &bob, &chip, &sign_at(edge), edge).unwrap();
}

#[test]
fn signature_is_bound_to_this_contract() {
    // Two deployments, same chip, both nonces still zero: a tap signed for
    // one can't be used on the other, because the contract address is signed.
    let s = setup();
    let chip = Chip::new(1);
    let alice = funded(&s, CARD_PRICE);
    let other = deploy(&s.e, &s.admin, &s.treasury, &s.usdc.address);
    s.card
        .register_card(&chip.id(&s.e), &CARD_PRICE, &track(&s.e), &s.admin);
    other.register_card(&chip.id(&s.e), &CARD_PRICE, &track(&s.e), &s.admin);

    let (sig, ts) = tap(&s, &chip, &alice); // signed for s.card
    assert_eq!(
        other.try_purchase(
            &alice,
            &chip.id(&s.e),
            &sig.rs,
            &sig.recovery_id,
            &ts,
            &no_extras(&s.e)
        ),
        Err(Ok(contract_err(CardError::InvalidSignature)))
    );
    purchase(&s, &alice, &chip, &sig, ts).unwrap();
}

#[test]
fn wallet_transfers_are_disabled() {
    let s = setup();
    let alice = funded(&s, CARD_PRICE);
    let bob = Address::generate(&s.e);
    let token_id = register_and_buy(&s, &Chip::new(1), &alice);

    assert_eq!(
        s.card.try_transfer(&alice, &bob, &token_id),
        Err(Ok(contract_err(CardError::TransferRequiresChip)))
    );
    assert_eq!(
        s.card.try_transfer_from(&alice, &alice, &bob, &token_id),
        Err(Ok(contract_err(CardError::TransferRequiresChip)))
    );
    assert_eq!(s.card.owner_of(&token_id), alice);
}

#[test]
fn transfer_token_needs_a_minted_card() {
    let s = setup();
    let chip = Chip::new(1);
    let bob = Address::generate(&s.e);
    s.card
        .register_card(&chip.id(&s.e), &CARD_PRICE, &track(&s.e), &s.admin);
    assert_eq!(
        s.card.try_token_id_for_chip(&chip.id(&s.e)),
        Err(Ok(contract_err(CardError::NoMappedTokenForChip)))
    );
    let (sig, ts) = tap(&s, &chip, &bob);
    assert_eq!(
        transfer_token(&s, &bob, &chip, &sig, ts),
        Err(contract_err(CardError::NoMappedTokenForChip))
    );
}

// --------------------------------------------------------- signature format

#[test]
fn recovery_id_must_be_0_or_1() {
    let s = setup();
    let chip = Chip::new(1);
    let alice = funded(&s, CARD_PRICE);
    let bob = Address::generate(&s.e);
    register_and_buy(&s, &chip, &alice);

    let (mut sig, ts) = tap(&s, &chip, &bob);
    sig.recovery_id += 27; // HaLo's raw `v` must be converted: recovery_id = v − 27
    assert_eq!(
        transfer_token(&s, &bob, &chip, &sig, ts),
        Err(contract_err(CardError::InvalidSignatureFormat))
    );
}

#[test]
fn is_chip_signature_for_token() {
    let s = setup();
    let chip = Chip::new(1);
    let token_id = register_and_buy(&s, &chip, &funded(&s, CARD_PRICE));

    let data = to_vec(&verify_data(&s.e, b"nonce 8f3a"));
    let other = to_vec(&verify_data(&s.e, b"something else"));
    let check = |sig: &Sig, data: &[u8]| {
        s.card.is_chip_signature_for_token(
            &token_id,
            &Bytes::from_slice(&s.e, data),
            &sig.rs,
            &sig.recovery_id,
        )
    };

    assert!(check(&chip.sign(&s.e, &data), &data));
    assert!(!check(&Chip::new(2).sign(&s.e, &data), &data));
    assert!(!check(&chip.sign(&s.e, &data), &other));
}

#[test]
fn verification_data_needs_the_verify_tag() {
    let s = setup();
    let chip = Chip::new(1);
    let token_id = register_and_buy(&s, &chip, &funded(&s, CARD_PRICE));
    let bob = Address::generate(&s.e);

    // Untagged data, and a transfer message passed off as a challenge.
    let ts = s.e.ledger().timestamp();
    let transfer_msg = s
        .card
        .signature_message(&bob, &chip.id(&s.e), &ts, &no_extras(&s.e));
    for data in [Bytes::from_slice(&s.e, b"nonce 8f3a"), transfer_msg] {
        let sig = chip.sign(&s.e, &to_vec(&data));
        assert_eq!(
            s.card
                .try_is_chip_signature_for_token(&token_id, &data, &sig.rs, &sig.recovery_id),
            Err(Ok(contract_err(CardError::InvalidVerificationData)))
        );
    }
}

#[test]
fn verification_signature_cant_move_the_token() {
    // Mallory's "is your card genuine?" page hands the holder a challenge
    // that is really the transfer message naming Mallory. Signed as
    // verification data it carries the verify tag, so it never matches the
    // transfer message, which carries the transfer tag.
    let s = setup();
    let chip = Chip::new(1);
    let alice = funded(&s, CARD_PRICE);
    let mallory = Address::generate(&s.e);
    let token_id = register_and_buy(&s, &chip, &alice);

    let ts = s.e.ledger().timestamp();
    let transfer_msg = s
        .card
        .signature_message(&mallory, &chip.id(&s.e), &ts, &no_extras(&s.e));
    assert!(to_vec(&transfer_msg).starts_with(TRANSFER_TAG));
    let challenge = verify_data(&s.e, &to_vec(&transfer_msg));
    let sig = chip.sign(&s.e, &to_vec(&challenge));
    assert!(s
        .card
        .is_chip_signature_for_token(&token_id, &challenge, &sig.rs, &sig.recovery_id));

    assert_eq!(
        transfer_token(&s, &mallory, &chip, &sig, ts),
        Err(contract_err(CardError::InvalidSignature))
    );
    assert_eq!(s.card.owner_of(&token_id), alice);
}

#[test]
fn verification_of_unknown_token_fails() {
    let s = setup();
    let chip = Chip::new(1);
    let data = verify_data(&s.e, b"nonce");
    let sig = chip.sign(&s.e, &to_vec(&data));
    assert_eq!(
        s.card
            .try_is_chip_signature_for_token(&7, &data, &sig.rs, &sig.recovery_id),
        Err(Ok(contract_err(CardError::UnknownToken)))
    );
}

#[test]
fn enumerates_multiple_cards() {
    let s = setup();
    let alice = funded(&s, 3 * CARD_PRICE);
    let ids: std::vec::Vec<u32> = (1..=3)
        .map(|i| register_and_buy(&s, &Chip::new(i), &alice))
        .collect();
    assert_eq!(s.card.total_supply(), 3);
    for (i, id) in ids.iter().enumerate() {
        assert_eq!(s.card.get_owner_token_id(&alice, &(i as u32)), *id);
    }
}

// ------------------------------------------------------------- tap to play

#[test]
fn sold_card_plays_for_anyone() {
    let s = setup();
    let chip = Chip::new(1);
    let alice = funded(&s, CARD_PRICE);
    let bob = Address::generate(&s.e);
    s.card
        .register_card(&chip.id(&s.e), &CARD_PRICE, &track(&s.e), &s.admin);

    // Unsold: a tap shows the price, not the track.
    assert_eq!(
        s.card.try_music_url(&chip.id(&s.e)),
        Err(Ok(contract_err(CardError::NoMappedTokenForChip)))
    );

    let (sig, ts) = tap(&s, &chip, &alice);
    purchase(&s, &alice, &chip, &sig, ts).unwrap();
    assert_eq!(s.card.music_url(&chip.id(&s.e)), track(&s.e));

    // The track follows the card, not the owner: still plays after a transfer.
    let (sig, ts) = tap(&s, &chip, &bob);
    transfer_token(&s, &bob, &chip, &sig, ts).unwrap();
    assert_eq!(s.card.music_url(&chip.id(&s.e)), track(&s.e));
}

#[test]
fn register_needs_an_http_music_url() {
    let s = setup();
    let id = Chip::new(1).id(&s.e);
    for bad in [
        "",
        "http://",
        "ipfs://bafy",
        "star-dust.mp3",
        "javascript:alert(1)",
    ] {
        assert_eq!(
            s.card
                .try_register_card(&id, &CARD_PRICE, &String::from_str(&s.e, bad), &s.admin),
            Err(Ok(contract_err(CardError::InvalidMusicUrl))),
            "{bad}"
        );
    }
    s.card.register_card(
        &id,
        &CARD_PRICE,
        &String::from_str(&s.e, "https://example.com/a.mp3"),
        &s.admin,
    );
    s.card
        .register_card(&Chip::new(2).id(&s.e), &CARD_PRICE, &track(&s.e), &s.admin);
}

#[test]
#[should_panic(expected = "Error(Contract, #3012)")]
fn zero_signature_window_is_rejected() {
    let e = Env::default();
    let admin = Address::generate(&e);
    e.register(
        RecordPoolSmartCard,
        (
            admin.clone(),
            Address::generate(&e),
            admin,
            0u64,
            String::from_str(&e, "http://localhost:8000/meta/"),
            String::from_str(&e, "RecordPool Smart Card"),
            String::from_str(&e, "RPCARD"),
        ),
    );
}

// ------------------------------------------------------------ owner actions

#[test]
fn transfer_to_current_owner_keeps_the_card() {
    let s = setup();
    let chip = Chip::new(1);
    let alice = funded(&s, CARD_PRICE);
    let token_id = register_and_buy(&s, &chip, &alice);

    let (sig, ts) = tap(&s, &chip, &alice);
    assert_eq!(transfer_token(&s, &alice, &chip, &sig, ts), Ok(token_id));
    assert_eq!(s.card.owner_of(&token_id), alice);
    assert_eq!(s.card.balance(&alice), 1);
    assert_eq!(s.card.get_owner_token_id(&alice, &0), token_id);
    // The signature was still used up.
    assert_eq!(
        transfer_token(&s, &alice, &chip, &sig, ts),
        Err(contract_err(CardError::InvalidSignature))
    );
}

#[test]
fn owner_can_cancel_unused_signatures() {
    let s = setup();
    let chip = Chip::new(1);
    let alice = funded(&s, CARD_PRICE);
    let mallory = Address::generate(&s.e);
    let token_id = register_and_buy(&s, &chip, &alice);

    // Mallory borrows the card and has it sign a transfer to herself.
    let (sig, ts) = tap(&s, &chip, &mallory);
    let before = s.card.chip_nonce(&chip.id(&s.e));

    s.card.rotate_nonce(&chip.id(&s.e));
    let auths = s.e.auths();
    assert_eq!(auths.len(), 1);
    assert_eq!(auths[0].0, alice);
    assert_eq!(
        s.e.events()
            .all()
            .filter_by_contract(&s.card.address)
            .events(),
        [NonceRotated {
            chip_id: chip.id(&s.e)
        }
        .to_xdr(&s.e, &s.card.address)]
    );
    assert_ne!(s.card.chip_nonce(&chip.id(&s.e)), before);

    assert_eq!(
        transfer_token(&s, &mallory, &chip, &sig, ts),
        Err(contract_err(CardError::InvalidSignature))
    );
    assert_eq!(s.card.owner_of(&token_id), alice);
}

#[test]
fn rotate_nonce_needs_the_owner() {
    let s = setup();
    let chip = Chip::new(1);
    let alice = funded(&s, CARD_PRICE);
    register_and_buy(&s, &chip, &alice);
    s.e.set_auths(&[]);
    assert!(s.card.try_rotate_nonce(&chip.id(&s.e)).is_err());
}

#[test]
fn rotate_nonce_needs_a_sold_card() {
    let s = setup();
    let chip = Chip::new(1);
    s.card
        .register_card(&chip.id(&s.e), &CARD_PRICE, &track(&s.e), &s.admin);
    assert_eq!(
        s.card.try_rotate_nonce(&chip.id(&s.e)),
        Err(Ok(contract_err(CardError::NoMappedTokenForChip)))
    );
}

// ------------------------------------------------------------------ admin

#[test]
fn issuer_sets_the_treasury() {
    let s = setup();
    let new_treasury = Address::generate(&s.e);
    let rando = Address::generate(&s.e);
    assert_eq!(
        s.card.try_set_treasury(&new_treasury, &rando),
        Err(Ok(Error::from_contract_error(ACCESS_UNAUTHORIZED)))
    );
    assert_eq!(s.card.config().treasury, s.treasury);

    s.card.set_treasury(&new_treasury, &s.admin);
    assert_eq!(s.card.config().treasury, new_treasury);

    // The next sale pays the new treasury.
    let buyer = funded(&s, CARD_PRICE);
    register_and_buy(&s, &Chip::new(1), &buyer);
    assert_eq!(s.usdc.balance(&new_treasury), CARD_PRICE);
    assert_eq!(s.usdc.balance(&s.treasury), 0);
}

#[test]
fn signature_is_bound_to_the_network() {
    let s = setup();
    let chip = Chip::new(1);
    let alice = funded(&s, CARD_PRICE);
    s.card
        .register_card(&chip.id(&s.e), &CARD_PRICE, &track(&s.e), &s.admin);

    let (sig, ts) = tap(&s, &chip, &alice); // signed for this network
    s.e.ledger().with_mut(|l| l.network_id = [7; 32]);
    assert_eq!(
        purchase(&s, &alice, &chip, &sig, ts),
        Err(contract_err(CardError::InvalidSignature))
    );
}

// ----------------------------------------------------------------- events

fn events(s: &Setup) -> std::vec::Vec<soroban_sdk::xdr::ContractEvent> {
    s.e.events()
        .all()
        .filter_by_contract(&s.card.address)
        .events()
        .to_vec()
}

#[test]
fn state_changes_emit_events() {
    let s = setup();
    let chip = Chip::new(1);
    let chip_id = chip.id(&s.e);
    let alice = funded(&s, CARD_PRICE);
    let bob = Address::generate(&s.e);
    let addr = s.card.address.clone();

    s.card
        .register_card(&chip_id, &CARD_PRICE, &track(&s.e), &s.admin);
    assert_eq!(
        events(&s),
        [CardRegistered {
            chip_id: chip_id.clone(),
            price: CARD_PRICE
        }
        .to_xdr(&s.e, &addr)]
    );

    let (sig, ts) = tap(&s, &chip, &alice);
    let token_id = purchase(&s, &alice, &chip, &sig, ts).unwrap();
    let emitted = events(&s);
    for expected in [
        Mint {
            to: alice.clone(),
            token_id,
        }
        .to_xdr(&s.e, &addr),
        ChipSet {
            token_id,
            chip_id: chip_id.clone(),
        }
        .to_xdr(&s.e, &addr),
        CardPurchased {
            buyer: alice.clone(),
            token_id,
            chip_id: chip_id.clone(),
            price: CARD_PRICE,
        }
        .to_xdr(&s.e, &addr),
    ] {
        assert!(emitted.contains(&expected), "missing {expected:?}");
    }
    // ChipSet's topics are ["chip_set", token_id, chip_id].
    let chip_set = ChipSet {
        token_id,
        chip_id: chip_id.clone(),
    };
    assert_eq!(
        chip_set.topics(&s.e),
        (Symbol::new(&s.e, "chip_set"), token_id, chip_id.clone()).into_val(&s.e)
    );

    let (sig, ts) = tap(&s, &chip, &bob);
    transfer_token(&s, &bob, &chip, &sig, ts).unwrap();
    assert_eq!(
        events(&s),
        [Transfer {
            from: alice,
            to: bob,
            token_id
        }
        .to_xdr(&s.e, &addr)]
    );
}

// ------------------------------------------------------------ storage TTL

fn instance_ttl(s: &Setup) -> u32 {
    s.e.as_contract(&s.card.address, || s.e.storage().instance().get_ttl())
}

/// Moves the ledger on two days, so every 30-day TTL is below its threshold.
fn age_two_days(s: &Setup) {
    let elapsed = INSTANCE_EXTEND_AMOUNT - INSTANCE_TTL_THRESHOLD + 17_280;
    s.e.ledger().with_mut(|l| l.sequence_number += elapsed);
    assert!(instance_ttl(s) < INSTANCE_TTL_THRESHOLD);
}

#[test]
fn each_state_change_extends_instance_ttl() {
    let s = setup();
    // The constructor extends the instance.
    assert_eq!(instance_ttl(&s), INSTANCE_EXTEND_AMOUNT);

    let chip = Chip::new(1);
    let alice = funded(&s, CARD_PRICE);
    let bob = Address::generate(&s.e);

    age_two_days(&s);
    s.card
        .register_card(&chip.id(&s.e), &CARD_PRICE, &track(&s.e), &s.admin);
    assert_eq!(instance_ttl(&s), INSTANCE_EXTEND_AMOUNT, "register_card");

    age_two_days(&s);
    let (sig, ts) = tap(&s, &chip, &alice);
    purchase(&s, &alice, &chip, &sig, ts).unwrap();
    assert_eq!(instance_ttl(&s), INSTANCE_EXTEND_AMOUNT, "purchase");

    age_two_days(&s);
    let (sig, ts) = tap(&s, &chip, &bob);
    transfer_token(&s, &bob, &chip, &sig, ts).unwrap();
    assert_eq!(instance_ttl(&s), INSTANCE_EXTEND_AMOUNT, "transfer_token");

    age_two_days(&s);
    s.card.rotate_nonce(&chip.id(&s.e));
    assert_eq!(instance_ttl(&s), INSTANCE_EXTEND_AMOUNT, "rotate_nonce");

    age_two_days(&s);
    s.card.set_treasury(&s.treasury, &s.admin);
    assert_eq!(instance_ttl(&s), INSTANCE_EXTEND_AMOUNT, "set_treasury");
}

#[test]
fn transfer_keeps_the_card_entries_live() {
    let s = setup();
    let chip = Chip::new(1);
    let alice = funded(&s, CARD_PRICE);
    let bob = Address::generate(&s.e);
    let token_id = register_and_buy(&s, &chip, &alice);

    age_two_days(&s);
    let (sig, ts) = tap(&s, &chip, &bob);
    transfer_token(&s, &bob, &chip, &sig, ts).unwrap();

    let ttl = |key: CardKey| {
        s.e.as_contract(&s.card.address, || s.e.storage().persistent().get_ttl(&key))
    };
    assert_eq!(ttl(CardKey::Music(chip.id(&s.e))), PERSISTENT_EXTEND_AMOUNT);
    assert_eq!(ttl(CardKey::TokenChip(token_id)), PERSISTENT_EXTEND_AMOUNT);
    assert_eq!(
        ttl(CardKey::ChipToken(chip.id(&s.e))),
        PERSISTENT_EXTEND_AMOUNT
    );
    assert_eq!(
        ttl(CardKey::ChipNonce(chip.id(&s.e))),
        PERSISTENT_EXTEND_AMOUNT
    );
}
