//! # RecordPool Smart Card — a Physical Backed Token on Soroban
//!
//! A RecordPool card is an NFC chip card you tap on a phone to play music.
//! Each card carries a HaLo NFC chip holding a secp256k1 key pair that can't
//! be extracted. This contract brings Ethereum's ERC-5791 "Physical Backed
//! Token" model to Soroban, using Stellar conventions throughout: each token
//! is paired with one chip, and **whoever holds the card controls the token**.
//!
//! * **Primary sale** — the issuer registers a chip and a price; the buyer
//!   taps the card (the chip signs a mint to them) and pays in a Stellar
//!   stablecoin (USDC on testnet), atomically.
//! * **Tap to play** — each card has a music URL. `purchase` returns it,
//!   and [`RecordPoolSmartCard::music_url`] gives it to anyone who taps a sold
//!   card, no wallet needed.
//! * **Tap to transfer** — [`RecordPoolSmartCard::transfer_token`] moves a token
//!   to whoever presents a fresh chip signature naming them. Wallet-signed
//!   `transfer` / `transfer_from` are disabled.
//! * **Replay protection** — every chip signature covers this contract, the
//!   network, a per-chip nonce that changes after each use, the recipient and
//!   a timestamp that must fall within `max_duration_window`.
//! * **Authenticity checks** — [`RecordPoolSmartCard::is_chip_signature_for_token`]
//!   verifies a chip signature over arbitrary data, read-only.
//!
//! Chips sign the SEP-53 digest, `SHA-256("Stellar Signed Message:\n" ||
//! message)`, with their secp256k1 key; the contract checks it with the
//! native `secp256k1_recover` host function. A chip's id is SHA-256 of its
//! public key. The token standard itself (ownership, enumeration, roles) comes from
//! [OpenZeppelin Stellar Contracts](https://github.com/OpenZeppelin/stellar-contracts).
#![no_std]

mod contract;
mod errors;
mod events;
mod storage;

pub use contract::{RecordPoolSmartCard, RecordPoolSmartCardClient};
pub use errors::CardError;
pub use storage::{ChipId, Config, Purchase};

#[cfg(test)]
mod test;
