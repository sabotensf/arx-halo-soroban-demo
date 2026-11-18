use soroban_sdk::{contracttype, panic_with_error, Address, BytesN, Env, String};

use crate::errors::CardError;

/// ~30 days at 5s ledgers. Persistent entries are bumped whenever they are
/// touched so that live cards never get archived.
const DAY_IN_LEDGERS: u32 = 17_280;
pub const PERSISTENT_EXTEND_AMOUNT: u32 = 30 * DAY_IN_LEDGERS;
pub const PERSISTENT_TTL_THRESHOLD: u32 = PERSISTENT_EXTEND_AMOUNT - DAY_IN_LEDGERS;

/// The contract instance (config, OpenZeppelin metadata, the contract code
/// reference) gets the same 30 days, extended by every state-changing call.
pub const INSTANCE_EXTEND_AMOUNT: u32 = PERSISTENT_EXTEND_AMOUNT;
pub const INSTANCE_TTL_THRESHOLD: u32 = PERSISTENT_TTL_THRESHOLD;

/// A chip's id: SHA-256 of its uncompressed (65-byte, `04…`) secp256k1
/// public key. For HaLo chips, key slot 1: libhalo `get_pkeys` → `publicKeys["1"]`.
pub type ChipId = BytesN<32>;

/// Contract configuration, stored in instance storage.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Config {
    /// SEP-41 token used for primary sales (e.g. the USDC Stellar Asset Contract).
    pub payment_token: Address,
    /// Receives primary-sale proceeds.
    pub treasury: Address,
    /// How long, in seconds, a chip signature stays valid after its timestamp.
    pub max_duration_window: u64,
}

/// What `purchase` returns.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Purchase {
    pub token_id: u32,
    /// The track the card plays.
    pub music_url: String,
}

#[contracttype]
pub enum CardKey {
    Config,
    /// chip_id -> price (registered by the issuer, not yet sold)
    Pending(ChipId),
    /// chip_id -> token_id (sold and minted)
    ChipToken(ChipId),
    /// token_id -> chip_id
    TokenChip(u32),
    /// chip_id -> nonce the chip's next signature must cover
    ChipNonce(ChipId),
    /// chip_id -> URL of the track the card plays
    Music(ChipId),
}

fn bump(e: &Env, key: &CardKey) {
    e.storage()
        .persistent()
        .extend_ttl(key, PERSISTENT_TTL_THRESHOLD, PERSISTENT_EXTEND_AMOUNT);
}

/// Keeps the contract instance live. Neither this contract's storage helpers
/// nor OpenZeppelin extend instance storage, so state-changing entry points
/// call this.
pub fn extend_instance_ttl(e: &Env) {
    e.storage()
        .instance()
        .extend_ttl(INSTANCE_TTL_THRESHOLD, INSTANCE_EXTEND_AMOUNT);
}

pub fn set_config(e: &Env, config: &Config) {
    e.storage().instance().set(&CardKey::Config, config);
}

pub fn get_config(e: &Env) -> Config {
    e.storage()
        .instance()
        .get(&CardKey::Config)
        .expect("config is set in the constructor")
}

pub fn is_known_chip(e: &Env, chip_id: &ChipId) -> bool {
    let p = e.storage().persistent();
    p.has(&CardKey::Pending(chip_id.clone())) || p.has(&CardKey::ChipToken(chip_id.clone()))
}

pub fn set_pending(e: &Env, chip_id: &ChipId, price: i128) {
    let key = CardKey::Pending(chip_id.clone());
    e.storage().persistent().set(&key, &price);
    bump(e, &key);
}

pub fn get_pending(e: &Env, chip_id: &ChipId) -> Option<i128> {
    let key = CardKey::Pending(chip_id.clone());
    let price = e.storage().persistent().get(&key);
    if price.is_some() {
        bump(e, &key);
    }
    price
}

/// Moves a card from "pending" to "minted" and pairs chip <-> token.
pub fn bind_chip(e: &Env, chip_id: &ChipId, token_id: u32) {
    let p = e.storage().persistent();
    p.remove(&CardKey::Pending(chip_id.clone()));

    let chip_key = CardKey::ChipToken(chip_id.clone());
    p.set(&chip_key, &token_id);
    bump(e, &chip_key);

    let token_key = CardKey::TokenChip(token_id);
    p.set(&token_key, chip_id);
    bump(e, &token_key);
}

pub fn get_token_for_chip(e: &Env, chip_id: &ChipId) -> Option<u32> {
    let key = CardKey::ChipToken(chip_id.clone());
    let token = e.storage().persistent().get(&key);
    if token.is_some() {
        bump(e, &key);
    }
    token
}

pub fn get_chip_for_token(e: &Env, token_id: u32) -> ChipId {
    let key = CardKey::TokenChip(token_id);
    let Some(chip_id) = e.storage().persistent().get(&key) else {
        panic_with_error!(e, CardError::UnknownToken);
    };
    bump(e, &key);
    chip_id
}

/// Starts at all zeros, like ERC-5791's reference implementation.
pub fn get_chip_nonce(e: &Env, chip_id: &ChipId) -> BytesN<32> {
    e.storage()
        .persistent()
        .get(&CardKey::ChipNonce(chip_id.clone()))
        .unwrap_or_else(|| BytesN::from_array(e, &[0; 32]))
}

pub fn set_chip_nonce(e: &Env, chip_id: &ChipId, nonce: &BytesN<32>) {
    let key = CardKey::ChipNonce(chip_id.clone());
    e.storage().persistent().set(&key, nonce);
    bump(e, &key);
}

pub fn set_music_url(e: &Env, chip_id: &ChipId, url: &String) {
    let key = CardKey::Music(chip_id.clone());
    e.storage().persistent().set(&key, url);
    bump(e, &key);
}

pub fn get_music_url(e: &Env, chip_id: &ChipId) -> String {
    let key = CardKey::Music(chip_id.clone());
    let url = e
        .storage()
        .persistent()
        .get(&key)
        .expect("every registered card has a music URL");
    bump(e, &key);
    url
}
