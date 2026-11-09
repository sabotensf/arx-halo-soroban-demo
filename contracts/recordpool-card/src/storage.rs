use soroban_sdk::{contracttype, panic_with_error, Address, BytesN, Env};

use crate::errors::CardError;

/// ~30 days at 5s ledgers. Persistent entries are bumped whenever they are
/// touched so that live cards never get archived.
const DAY_IN_LEDGERS: u32 = 17_280;
pub const PERSISTENT_EXTEND_AMOUNT: u32 = 30 * DAY_IN_LEDGERS;
pub const PERSISTENT_TTL_THRESHOLD: u32 = PERSISTENT_EXTEND_AMOUNT - DAY_IN_LEDGERS;

/// Commerce configuration, stored in instance storage.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Config {
    /// SEP-41 token used for payments (e.g. the USDC Stellar Asset Contract).
    pub payment_token: Address,
    /// Receives primary-sale proceeds (and, by default, royalties).
    pub treasury: Address,
}

/// A card registered by the issuer that has not been sold yet.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingCard {
    /// Uncompressed SEC1 secp256k1 public key (HaLo key slot 2, `pk2`).
    pub chip_pubkey: BytesN<65>,
    /// Primary-sale price, in the payment token's smallest unit.
    pub price: i128,
}

/// A secondary-market listing.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Listing {
    pub seller: Address,
    pub price: i128,
}

#[contracttype]
pub enum CardKey {
    Config,
    /// chip_id -> PendingCard (registered, not yet sold)
    Pending(BytesN<32>),
    /// chip_id -> token_id (sold and minted)
    ChipToken(BytesN<32>),
    /// token_id -> chip public key
    TokenChip(u32),
    /// token_id -> Listing
    Listing(u32),
}

fn bump(e: &Env, key: &CardKey) {
    e.storage()
        .persistent()
        .extend_ttl(key, PERSISTENT_TTL_THRESHOLD, PERSISTENT_EXTEND_AMOUNT);
}

pub fn chip_id(e: &Env, chip_pubkey: &BytesN<65>) -> BytesN<32> {
    e.crypto().sha256(&chip_pubkey.clone().into()).into()
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

pub fn is_known_chip(e: &Env, chip_id: &BytesN<32>) -> bool {
    let p = e.storage().persistent();
    p.has(&CardKey::Pending(chip_id.clone())) || p.has(&CardKey::ChipToken(chip_id.clone()))
}

pub fn set_pending(e: &Env, chip_id: &BytesN<32>, card: &PendingCard) {
    let key = CardKey::Pending(chip_id.clone());
    e.storage().persistent().set(&key, card);
    bump(e, &key);
}

pub fn get_pending(e: &Env, chip_id: &BytesN<32>) -> Option<PendingCard> {
    let key = CardKey::Pending(chip_id.clone());
    let card = e.storage().persistent().get(&key);
    if card.is_some() {
        bump(e, &key);
    }
    card
}

/// Moves a card from "pending" to "minted" and binds chip <-> token.
pub fn bind_chip(e: &Env, chip_id: &BytesN<32>, chip_pubkey: &BytesN<65>, token_id: u32) {
    let p = e.storage().persistent();
    p.remove(&CardKey::Pending(chip_id.clone()));

    let chip_key = CardKey::ChipToken(chip_id.clone());
    p.set(&chip_key, &token_id);
    bump(e, &chip_key);

    let token_key = CardKey::TokenChip(token_id);
    p.set(&token_key, chip_pubkey);
    bump(e, &token_key);
}

pub fn get_token_for_chip(e: &Env, chip_id: &BytesN<32>) -> Option<u32> {
    let key = CardKey::ChipToken(chip_id.clone());
    let token = e.storage().persistent().get(&key);
    if token.is_some() {
        bump(e, &key);
    }
    token
}

pub fn get_chip_pubkey(e: &Env, token_id: u32) -> BytesN<65> {
    let key = CardKey::TokenChip(token_id);
    let Some(pubkey) = e.storage().persistent().get(&key) else {
        panic_with_error!(e, CardError::UnknownToken);
    };
    bump(e, &key);
    pubkey
}

pub fn set_listing(e: &Env, token_id: u32, listing: &Listing) {
    let key = CardKey::Listing(token_id);
    e.storage().persistent().set(&key, listing);
    bump(e, &key);
}

pub fn get_listing(e: &Env, token_id: u32) -> Option<Listing> {
    e.storage().persistent().get(&CardKey::Listing(token_id))
}

pub fn remove_listing(e: &Env, token_id: u32) {
    e.storage().persistent().remove(&CardKey::Listing(token_id));
}
