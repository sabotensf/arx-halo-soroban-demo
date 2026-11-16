//! # RecordPool Chipped Card — an RWA-backed NFT on Soroban
//!
//! A RecordPool card is an NFC chip card you tap on a phone to play music.
//! Each card carries either a HaLo NFC chip (a secp256k1 key pair that can't
//! be extracted) or an NXP NTAG 424 DNA chip (secret AES keys). This contract
//! binds each card's chip to exactly one on-chain token and adds the commerce
//! logic around it:
//!
//! * **Primary sale** — the issuer registers a card's chip public key and a
//!   price; a buyer pays in a Stellar stablecoin (USDC on testnet) and the
//!   token is minted to them atomically.
//! * **Authenticity (HaLo)** — anyone holding the physical card can have the
//!   chip sign a fresh challenge; [`RecordPoolCard::authenticate`] verifies it
//!   on-chain with the native `secp256k1_recover` host function and returns
//!   the owner.
//! * **Authenticity (NTAG 424 DNA)** — the chip's AES-CMAC can only be checked
//!   with its secret key, so an account with the `verifier` role checks taps
//!   off-chain and records them with [`RecordPoolCard::attest_tap`], which
//!   rejects replays using the chip's tap counter.
//! * **Secondary sale with royalties** — owners list a card, buyers pay
//!   through the contract, and the royalty from the OpenZeppelin `Royalties`
//!   extension is split out in the same transaction.
//!
//! The token standard itself (ownership, approvals, enumeration, royalty
//! bookkeeping, roles) comes from
//! [OpenZeppelin Stellar Contracts](https://github.com/OpenZeppelin/stellar-contracts).
#![no_std]

mod contract;
mod errors;
mod events;
mod storage;

pub use contract::{RecordPoolCard, RecordPoolCardClient};
pub use errors::CardError;
pub use storage::{Chip, Config, Listing};

#[cfg(test)]
mod test;
