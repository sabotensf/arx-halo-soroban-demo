use soroban_sdk::{
    contract, contractimpl, crypto::Hash, panic_with_error, symbol_short, token::TokenClient,
    xdr::ToXdr, Address, Bytes, BytesN, Env, String, Symbol, Vec,
};
use stellar_access::access_control::{self as access_control, AccessControl};
use stellar_macros::only_role;
use stellar_tokens::non_fungible::{
    emit_transfer,
    enumerable::{Enumerable, NonFungibleEnumerable},
    Base, NonFungibleToken,
};

use crate::{
    errors::CardError,
    events::{CardPurchased, CardRegistered, ChipSet},
    storage::{self, Config, Purchase},
};

/// Role allowed to register physical cards and set the treasury.
pub const ISSUER: Symbol = symbol_short!("issuer");

/// SEP-53 ("Sign and Verify Messages") prefix. Chips sign
/// `SHA-256(prefix || message)`, the same digest SEP-53 defines for Stellar
/// keys, with their secp256k1 key instead of Ed25519.
pub const SEP53_PREFIX: &[u8] = b"Stellar Signed Message:\n";

/// A Physical Backed Token for RecordPool tap-to-play music cards, modelled
/// on Ethereum's ERC-5791 and built with Stellar conventions.
///
/// Each token is paired with the HaLo NFC chip inside one physical card, and
/// **whoever holds the card controls the token**: minting and every transfer
/// require a fresh signature from the chip, and wallet-signed `transfer` /
/// `transfer_from` are disabled.
#[contract]
pub struct RecordPoolSmartCard;

#[contractimpl]
impl RecordPoolSmartCard {
    /// Deploy-time initialisation (Soroban constructors run atomically with
    /// deployment, so there is no separate `initialize` to front-run).
    ///
    /// * `admin` — receives the OpenZeppelin AccessControl admin role and the
    ///   `issuer` role.
    /// * `payment_token` — SEP-41 token for primary sales (testnet USDC SAC).
    /// * `treasury` — receives primary-sale proceeds.
    /// * `max_duration_window` — seconds a chip signature stays valid.
    #[allow(clippy::too_many_arguments)]
    pub fn __constructor(
        e: &Env,
        admin: Address,
        payment_token: Address,
        treasury: Address,
        max_duration_window: u64,
        base_uri: String,
        name: String,
        symbol: String,
    ) {
        Base::set_metadata(e, base_uri, name, symbol);

        access_control::set_admin(e, &admin);
        access_control::grant_role_no_auth(e, &admin, &ISSUER, &admin);

        storage::set_config(
            e,
            &Config {
                payment_token,
                treasury,
                max_duration_window,
            },
        );
        storage::extend_instance_ttl(e);
    }

    // ------------------------------------------------------------------
    // Physical Backed Token interface
    // ------------------------------------------------------------------

    /// Token paired with `chip_id`. Panics with `NoMappedTokenForChip` if the
    /// card hasn't been sold yet.
    pub fn token_id_for_chip(e: &Env, chip_id: BytesN<32>) -> u32 {
        storage::get_token_for_chip(e, &chip_id)
            .unwrap_or_else(|| panic_with_error!(e, CardError::NoMappedTokenForChip))
    }

    /// True if the paired chip signed `data`, i.e. signed
    /// `SHA-256("Stellar Signed Message:\n" || data)`. Read-only: run it as a
    /// simulation.
    ///
    /// Signatures that can't be recovered at all make the host trap instead
    /// of returning false.
    pub fn is_chip_signature_for_token(
        e: &Env,
        token_id: u32,
        data: Bytes,
        chip_signature: BytesN<64>,
        recovery_id: u32,
    ) -> bool {
        let chip_id = storage::get_chip_for_token(e, token_id);
        let digest = sep53_digest(e, &data);
        recover_chip_id(e, &digest, &chip_signature, recovery_id) == chip_id
    }

    /// Moves the token paired with `chip_id` from its current owner to `to`.
    ///
    /// Authorised only by the chip: `chip_signature` must sign
    /// [`signature_message`](Self::signature_message) for this `to`,
    /// `signature_timestamp` and `extras`, and the timestamp must be within
    /// `max_duration_window` of the ledger time. Each signature works once,
    /// because the chip's nonce changes after every use. No wallet signature
    /// is needed, so anyone (typically `to`) can submit the transaction.
    pub fn transfer_token(
        e: &Env,
        to: Address,
        chip_id: BytesN<32>,
        chip_signature: BytesN<64>,
        recovery_id: u32,
        signature_timestamp: u64,
        extras: Bytes,
    ) -> u32 {
        storage::extend_instance_ttl(e);
        let token_id = Self::token_id_for_chip(e, chip_id.clone());
        consume_chip_signature(
            e,
            &chip_id,
            &to,
            &chip_signature,
            recovery_id,
            signature_timestamp,
            &extras,
        );

        let from = Self::owner_of(e, token_id);
        Base::update(e, Some(&from), Some(&to), token_id);
        Enumerable::remove_from_owner_enumeration(e, &from, token_id);
        Enumerable::add_to_owner_enumeration(e, &to, token_id);
        emit_transfer(e, &from, &to, token_id);
        token_id
    }

    // ------------------------------------------------------------------
    // Signing helpers (views)
    // ------------------------------------------------------------------

    /// The 32-byte message the chip must sign for `transfer_token` or
    /// `purchase`:
    ///
    /// ```text
    /// SHA-256( contract || network_id || chip_id || chip_nonce || to
    ///          || signature_timestamp (u64, big-endian) || SHA-256(extras) )
    /// ```
    /// where `contract` and `to` are XDR-encoded `ScVal` addresses. The chip
    /// signs it the SEP-53 way: the digest is
    /// `SHA-256("Stellar Signed Message:\n" || message)`, passed to HaLo's
    /// `sign` command as `digest` (key slot 1).
    pub fn signature_message(
        e: &Env,
        to: Address,
        chip_id: BytesN<32>,
        signature_timestamp: u64,
        extras: Bytes,
    ) -> BytesN<32> {
        let mut msg = e.current_contract_address().to_xdr(e);
        msg.append(&e.ledger().network_id().into());
        msg.append(&chip_id.clone().into());
        msg.append(&storage::get_chip_nonce(e, &chip_id).into());
        msg.append(&to.to_xdr(e));
        msg.append(&Bytes::from_array(e, &signature_timestamp.to_be_bytes()));
        msg.append(&e.crypto().sha256(&extras).into());
        e.crypto().sha256(&msg).into()
    }

    /// The nonce the chip's next signature must cover.
    pub fn chip_nonce(e: &Env, chip_id: BytesN<32>) -> BytesN<32> {
        storage::get_chip_nonce(e, &chip_id)
    }

    /// Chip paired with `token_id`.
    pub fn chip_id_for_token(e: &Env, token_id: u32) -> BytesN<32> {
        storage::get_chip_for_token(e, token_id)
    }

    // ------------------------------------------------------------------
    // Primary sale: issuer registers a chip, buyer taps the card and pays.
    // ------------------------------------------------------------------

    /// Adds a manufactured card to the sellable inventory. `chip_id` is
    /// SHA-256 of the chip's uncompressed key-slot-1 public key (libhalo
    /// `get_pkeys` → `publicKeys["1"]`). `music_url` is the track the card
    /// plays once it has been sold.
    #[only_role(operator, "issuer")]
    pub fn register_card(
        e: &Env,
        chip_id: BytesN<32>,
        price: i128,
        music_url: String,
        operator: Address,
    ) {
        storage::extend_instance_ttl(e);
        if price <= 0 {
            panic_with_error!(e, CardError::InvalidPrice);
        }
        if music_url.is_empty() {
            panic_with_error!(e, CardError::InvalidMusicUrl);
        }
        if storage::is_known_chip(e, &chip_id) {
            panic_with_error!(e, CardError::CardAlreadyRegistered);
        }
        storage::set_pending(e, &chip_id, price);
        storage::set_music_url(e, &chip_id, &music_url);
        CardRegistered { chip_id, price }.publish(e);
    }

    /// Buys a registered card. The buyer must hold it: the chip signs a mint
    /// to `buyer` (same message as `transfer_token`), the buyer's wallet
    /// authorises the USDC payment, and the token is minted and paired with
    /// the chip, all in one atomic transaction. Returns the new token id and
    /// the card's music URL.
    #[allow(clippy::too_many_arguments)]
    pub fn purchase(
        e: &Env,
        buyer: Address,
        chip_id: BytesN<32>,
        chip_signature: BytesN<64>,
        recovery_id: u32,
        signature_timestamp: u64,
        extras: Bytes,
    ) -> Purchase {
        buyer.require_auth();
        storage::extend_instance_ttl(e);

        let Some(price) = storage::get_pending(e, &chip_id) else {
            if storage::get_token_for_chip(e, &chip_id).is_some() {
                panic_with_error!(e, CardError::CardAlreadyMinted);
            }
            panic_with_error!(e, CardError::CardNotRegistered);
        };
        consume_chip_signature(
            e,
            &chip_id,
            &buyer,
            &chip_signature,
            recovery_id,
            signature_timestamp,
            &extras,
        );

        let config = storage::get_config(e);
        TokenClient::new(e, &config.payment_token).transfer(&buyer, &config.treasury, &price);

        let token_id = <Self as NonFungibleToken>::ContractType::mint(e, &buyer);
        storage::bind_chip(e, &chip_id, token_id);

        ChipSet {
            token_id,
            chip_id: chip_id.clone(),
        }
        .publish(e);
        let music_url = storage::get_music_url(e, &chip_id);
        CardPurchased {
            buyer,
            token_id,
            chip_id,
            price,
        }
        .publish(e);
        Purchase {
            token_id,
            music_url,
        }
    }

    /// The track a sold card plays. This is what a plain tap looks up: the
    /// tap page reads the chip's public key from the URL the chip opens,
    /// derives `chip_id` and calls this view, so anyone holding the card can
    /// play it without a wallet. Panics with `NoMappedTokenForChip` while the
    /// card is unsold.
    pub fn music_url(e: &Env, chip_id: BytesN<32>) -> String {
        Self::token_id_for_chip(e, chip_id.clone());
        storage::get_music_url(e, &chip_id)
    }

    /// Price of a card that is still for sale from the issuer.
    pub fn card_price(e: &Env, chip_id: BytesN<32>) -> Option<i128> {
        storage::get_pending(e, &chip_id)
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
        storage::extend_instance_ttl(e);
        let mut config = storage::get_config(e);
        config.treasury = treasury;
        storage::set_config(e, &config);
    }
}

// ----------------------------------------------------------------------
// Chip signatures
// ----------------------------------------------------------------------

/// SEP-53 digest: `SHA-256("Stellar Signed Message:\n" || message)`.
fn sep53_digest(e: &Env, message: &Bytes) -> Hash<32> {
    let mut prefixed = Bytes::from_slice(e, SEP53_PREFIX);
    prefixed.append(message);
    e.crypto().sha256(&prefixed)
}

/// Recovers the chip id (SHA-256 of the uncompressed public key) of the key
/// that made `signature` (64 bytes r || s).
fn recover_chip_id(
    e: &Env,
    digest: &Hash<32>,
    signature: &BytesN<64>,
    recovery_id: u32,
) -> BytesN<32> {
    if recovery_id > 1 {
        panic_with_error!(e, CardError::InvalidSignatureFormat);
    }
    let pubkey = e.crypto().secp256k1_recover(digest, signature, recovery_id);
    e.crypto().sha256(&pubkey.into()).into()
}

/// Checks a chip signature authorising a mint or transfer to `to`, then
/// rotates the chip's nonce so the signature can't be used again.
fn consume_chip_signature(
    e: &Env,
    chip_id: &BytesN<32>,
    to: &Address,
    signature: &BytesN<64>,
    recovery_id: u32,
    signature_timestamp: u64,
    extras: &Bytes,
) {
    let now = e.ledger().timestamp();
    if signature_timestamp > now {
        panic_with_error!(e, CardError::SignatureTimestampInFuture);
    }
    let window = storage::get_config(e).max_duration_window;
    if signature_timestamp.saturating_add(window) < now {
        panic_with_error!(e, CardError::SignatureTimestampTooOld);
    }

    let message = RecordPoolSmartCard::signature_message(
        e,
        to.clone(),
        chip_id.clone(),
        signature_timestamp,
        extras.clone(),
    );
    let digest = sep53_digest(e, &message.into());
    if recover_chip_id(e, &digest, signature, recovery_id) != *chip_id {
        panic_with_error!(e, CardError::InvalidSignature);
    }

    // ERC-5791 mixes in the previous block hash; Soroban contracts can't read
    // one, so mix in the per-invocation PRNG to keep the next nonce
    // unpredictable.
    let mut seed: Bytes = digest.to_bytes().into();
    seed.append(&e.prng().gen::<BytesN<32>>().into());
    storage::set_chip_nonce(e, chip_id, &e.crypto().sha256(&seed).into());
}

// ----------------------------------------------------------------------
// OpenZeppelin building blocks
// ----------------------------------------------------------------------

/// Core NFT (balance, owner_of, approve, metadata) with the Enumerable
/// extension. As for ERC-5791, the wallet-signed transfer functions are
/// disabled: tokens move only through `transfer_token`, with a chip signature.
#[contractimpl(contracttrait)]
impl NonFungibleToken for RecordPoolSmartCard {
    type ContractType = Enumerable;

    fn transfer(e: &Env, _from: Address, _to: Address, _token_id: u32) {
        panic_with_error!(e, CardError::TransferRequiresChip);
    }

    fn transfer_from(e: &Env, _spender: Address, _from: Address, _to: Address, _token_id: u32) {
        panic_with_error!(e, CardError::TransferRequiresChip);
    }
}

/// `total_supply`, `get_token_id`, `get_owner_token_id` — lets a wallet or
/// the RecordPool app list every card an account holds.
#[contractimpl(contracttrait)]
impl NonFungibleEnumerable for RecordPoolSmartCard {}

/// Role-based access control (admin, `issuer`), two-step admin transfer.
#[contractimpl(contracttrait)]
impl AccessControl for RecordPoolSmartCard {}
