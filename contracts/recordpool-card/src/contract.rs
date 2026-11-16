use soroban_sdk::{
    contract, contractimpl, panic_with_error, symbol_short, token::TokenClient, Address, Bytes,
    BytesN, Env, String, Symbol, Vec,
};
use stellar_access::access_control::{self as access_control, AccessControl};
use stellar_macros::only_role;
use stellar_tokens::non_fungible::{
    emit_transfer,
    enumerable::{Enumerable, NonFungibleEnumerable},
    royalties::{NonFungibleRoyalties, Royalties, RoyaltySupport},
    Base, Compose, NonFungibleToken,
};

use crate::{
    errors::CardError,
    events::{CardListed, CardPurchased, CardRegistered, CardResold, ListingCancelled},
    storage::{self, Config, Listing, PendingCard},
};

/// Domain-separation prefix a HaLo chip applies in `sign_challenge` (key slot 2).
pub const HALO_CHALLENGE_PREFIX: &[u8] = b"\x19External chall pk2:\n";

/// Role allowed to register physical cards and manage royalties.
pub const ISSUER: Symbol = symbol_short!("issuer");

#[contract]
pub struct RecordPoolCard;

#[contractimpl]
impl RecordPoolCard {
    /// Deploy-time initialisation (Soroban constructors run atomically with
    /// deployment, so there is no separate `initialize` to front-run).
    ///
    /// * `admin` — receives the OpenZeppelin AccessControl admin role and the
    ///   `issuer` role.
    /// * `payment_token` — SEP-41 token for all payments (testnet USDC SAC).
    /// * `treasury` — receives primary sales and the default royalty.
    /// * `royalty_bps` — default secondary-sale royalty, in basis points.
    #[allow(clippy::too_many_arguments)]
    pub fn __constructor(
        e: &Env,
        admin: Address,
        payment_token: Address,
        treasury: Address,
        royalty_bps: u32,
        base_uri: String,
        name: String,
        symbol: String,
    ) {
        Base::set_metadata(e, base_uri, name, symbol);
        Base::set_default_royalty(e, &treasury, royalty_bps);

        access_control::set_admin(e, &admin);
        access_control::grant_role_no_auth(e, &admin, &ISSUER, &admin);

        storage::set_config(
            e,
            &Config {
                payment_token,
                treasury,
            },
        );
    }

    // ------------------------------------------------------------------
    // Primary sale: issuer registers a chip, buyer pays and mints.
    // ------------------------------------------------------------------

    /// Adds a manufactured card to the sellable inventory. Returns its
    /// `chip_id` (SHA-256 of the chip public key), which is what gets printed
    /// in the card's NFC NDEF record / QR code.
    #[only_role(operator, "issuer")]
    pub fn register_card(
        e: &Env,
        chip_pubkey: BytesN<65>,
        price: i128,
        operator: Address,
    ) -> BytesN<32> {
        if price <= 0 {
            panic_with_error!(e, CardError::InvalidPrice);
        }
        let chip_id = storage::chip_id(e, &chip_pubkey);
        if storage::is_known_chip(e, &chip_id) {
            panic_with_error!(e, CardError::CardAlreadyRegistered);
        }
        storage::set_pending(e, &chip_id, &PendingCard { chip_pubkey, price });
        CardRegistered {
            chip_id: chip_id.clone(),
            price,
        }
        .publish(e);
        chip_id
    }

    /// Buys a registered card: pays the issuer's price in the payment token
    /// to the treasury and mints the card's token to `buyer`, in one atomic
    /// transaction. Returns the new token id.
    pub fn purchase(e: &Env, buyer: Address, chip_id: BytesN<32>) -> u32 {
        buyer.require_auth();

        let Some(card) = storage::get_pending(e, &chip_id) else {
            if storage::get_token_for_chip(e, &chip_id).is_some() {
                panic_with_error!(e, CardError::CardAlreadyMinted);
            }
            panic_with_error!(e, CardError::CardNotRegistered);
        };

        let config = storage::get_config(e);
        TokenClient::new(e, &config.payment_token).transfer(&buyer, &config.treasury, &card.price);

        let token_id = <Self as NonFungibleToken>::ContractType::mint(e, &buyer);
        storage::bind_chip(e, &chip_id, &card.chip_pubkey, token_id);

        CardPurchased {
            buyer,
            token_id,
            chip_id,
            price: card.price,
        }
        .publish(e);
        token_id
    }

    /// Price of a card that is still for sale from the issuer.
    pub fn card_price(e: &Env, chip_id: BytesN<32>) -> Option<i128> {
        storage::get_pending(e, &chip_id).map(|c| c.price)
    }

    /// Token bound to a chip, once sold.
    pub fn token_for_chip(e: &Env, chip_id: BytesN<32>) -> Option<u32> {
        storage::get_token_for_chip(e, &chip_id)
    }

    /// Public key of the chip bound to `token_id`.
    pub fn chip_pubkey(e: &Env, token_id: u32) -> BytesN<65> {
        storage::get_chip_pubkey(e, token_id)
    }

    // ------------------------------------------------------------------
    // Physical <-> digital link
    // ------------------------------------------------------------------

    /// Proves a physical card is genuine.
    ///
    /// RecordPool cards carry a HaLo NFC chip whose key slot 2 holds a
    /// secp256k1 key that never leaves the chip. The verifier generates a fresh
    /// random 32-byte `challenge` and has the chip sign it with libhalo's
    /// `sign_challenge`, which signs
    /// `SHA-256("\x19External chall pk2:\n" || challenge)`. The contract
    /// recomputes that digest, recovers the signer with the native
    /// `secp256k1_recover` host function, and checks it against the chip key
    /// bound to `token_id`. Panics if it doesn't match; otherwise returns the
    /// current on-chain owner.
    ///
    /// `signature` is r||s with a low S value and `recovery_id` is 0 or 1;
    /// `halo-tool prepare` converts HaLo's DER output into that form.
    ///
    /// Read-only: call it via simulation, no transaction or fee required.
    pub fn authenticate(
        e: &Env,
        token_id: u32,
        challenge: BytesN<32>,
        signature: BytesN<64>,
        recovery_id: u32,
    ) -> Address {
        let pubkey = storage::get_chip_pubkey(e, token_id);
        let mut msg = Bytes::from_slice(e, HALO_CHALLENGE_PREFIX);
        msg.append(&challenge.into());
        let digest = e.crypto().sha256(&msg);
        let signer = e
            .crypto()
            .secp256k1_recover(&digest, &signature, recovery_id);
        if signer != pubkey {
            panic_with_error!(e, CardError::InvalidChipSignature);
        }
        Self::owner_of(e, token_id)
    }

    // ------------------------------------------------------------------
    // Secondary market with enforced royalties
    // ------------------------------------------------------------------

    /// Lists a card for resale. Only the current owner may list.
    pub fn list(e: &Env, seller: Address, token_id: u32, price: i128) {
        seller.require_auth();
        if price <= 0 {
            panic_with_error!(e, CardError::InvalidPrice);
        }
        if Self::owner_of(e, token_id) != seller {
            panic_with_error!(e, CardError::NotTokenOwner);
        }
        storage::set_listing(
            e,
            token_id,
            &Listing {
                seller: seller.clone(),
                price,
            },
        );
        CardListed {
            seller,
            token_id,
            price,
        }
        .publish(e);
    }

    /// Cancels an active listing.
    pub fn cancel_listing(e: &Env, seller: Address, token_id: u32) {
        seller.require_auth();
        let listing = Self::active_listing(e, token_id);
        if listing.seller != seller {
            panic_with_error!(e, CardError::NotTokenOwner);
        }
        storage::remove_listing(e, token_id);
        ListingCancelled { token_id }.publish(e);
    }

    pub fn get_listing(e: &Env, token_id: u32) -> Option<Listing> {
        storage::get_listing(e, token_id)
    }

    /// Buys a listed card. In one transaction the buyer pays the royalty to
    /// the royalty receiver, the remainder to the seller, and receives the
    /// token. The royalty is therefore *enforced*, not merely advertised.
    pub fn buy(e: &Env, buyer: Address, token_id: u32) {
        buyer.require_auth();
        let listing = Self::active_listing(e, token_id);
        let seller = listing.seller;
        if seller == buyer {
            panic_with_error!(e, CardError::SelfPurchase);
        }

        let (royalty_receiver, royalty) =
            <Self as NonFungibleToken>::ContractType::royalty_info(e, token_id, listing.price);
        if royalty > listing.price {
            panic_with_error!(e, CardError::RoyaltyExceedsPrice);
        }

        let token = TokenClient::new(e, &storage::get_config(e).payment_token);
        if royalty > 0 {
            token.transfer(&buyer, &royalty_receiver, &royalty);
        }
        token.transfer(&buyer, &seller, &(listing.price - royalty));

        // The seller authorised this transfer when they created the listing,
        // so move the token with OpenZeppelin's low-level primitives (which
        // also clear any outstanding approval) instead of re-requiring their
        // signature, keeping enumeration bookkeeping consistent.
        storage::remove_listing(e, token_id);
        Base::update(e, Some(&seller), Some(&buyer), token_id);
        Enumerable::remove_from_owner_enumeration(e, &seller, token_id);
        Enumerable::add_to_owner_enumeration(e, &buyer, token_id);
        emit_transfer(e, &seller, &buyer, token_id);

        CardResold {
            seller,
            buyer,
            token_id,
            price: listing.price,
            royalty_receiver,
            royalty,
        }
        .publish(e);
    }

    // ------------------------------------------------------------------
    // Admin
    // ------------------------------------------------------------------

    pub fn config(e: &Env) -> Config {
        storage::get_config(e)
    }

    /// Points primary-sale proceeds at a new treasury.
    #[only_role(operator, "issuer")]
    pub fn set_treasury(e: &Env, treasury: Address, operator: Address) {
        let mut config = storage::get_config(e);
        config.treasury = treasury;
        storage::set_config(e, &config);
    }

    // ------------------------------------------------------------------
    // Internal helpers
    // ------------------------------------------------------------------

    /// Returns the listing if it exists and the seller still owns the token.
    fn active_listing(e: &Env, token_id: u32) -> Listing {
        let Some(listing) = storage::get_listing(e, token_id) else {
            panic_with_error!(e, CardError::NotListed);
        };
        if Self::owner_of(e, token_id) != listing.seller {
            // Defensive: transfers clear listings, so this should not happen.
            panic_with_error!(e, CardError::NotListed);
        }
        listing
    }

    fn clear_listing_on_transfer(e: &Env, token_id: u32) {
        if storage::get_listing(e, token_id).is_some() {
            storage::remove_listing(e, token_id);
            ListingCancelled { token_id }.publish(e);
        }
    }
}

// ----------------------------------------------------------------------
// OpenZeppelin building blocks
// ----------------------------------------------------------------------

/// Core NFT (balance, owner_of, transfer, approve, metadata) composed with
/// the Enumerable and Royalties extensions. Transfers are wrapped so that a
/// card moving to a new owner can never leave a stale resale listing behind.
#[contractimpl(contracttrait)]
impl NonFungibleToken for RecordPoolCard {
    type ContractType = Compose<(Enumerable, Royalties)>;

    fn transfer(e: &Env, from: Address, to: Address, token_id: u32) {
        Self::clear_listing_on_transfer(e, token_id);
        Self::ContractType::transfer(e, &from, &to, token_id);
    }

    fn transfer_from(e: &Env, spender: Address, from: Address, to: Address, token_id: u32) {
        Self::clear_listing_on_transfer(e, token_id);
        Self::ContractType::transfer_from(e, &spender, &from, &to, token_id);
    }
}

/// `total_supply`, `get_token_id`, `get_owner_token_id` — lets a wallet or
/// the RecordPool app list every card an account holds.
#[contractimpl(contracttrait)]
impl NonFungibleEnumerable for RecordPoolCard {}

/// ERC-2981-style royalty info, managed by the issuer role.
#[contractimpl(contracttrait)]
impl NonFungibleRoyalties for RecordPoolCard {
    #[only_role(operator, "issuer")]
    fn set_default_royalty(e: &Env, receiver: Address, basis_points: u32, operator: Address) {
        Base::set_default_royalty(e, &receiver, basis_points);
    }

    #[only_role(operator, "issuer")]
    fn set_token_royalty(
        e: &Env,
        token_id: u32,
        receiver: Address,
        basis_points: u32,
        operator: Address,
    ) {
        Self::ContractType::set_token_royalty(e, token_id, &receiver, basis_points);
    }

    #[only_role(operator, "issuer")]
    fn remove_token_royalty(e: &Env, token_id: u32, operator: Address) {
        Self::ContractType::remove_token_royalty(e, token_id);
    }
}

/// Role-based access control (admin, `issuer`), two-step admin transfer.
#[contractimpl(contracttrait)]
impl AccessControl for RecordPoolCard {}
