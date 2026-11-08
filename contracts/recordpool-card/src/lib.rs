//! # RecordPool Chipped Card — an RWA-backed NFT on Soroban
//!
//! Every physical RecordPool card carries an NFC secure element holding a
//! secp256r1 (P-256) key pair. This contract binds each card's chip to exactly
//! one on-chain token and adds the commerce logic around it:
//!
//! * **Primary sale** — the issuer registers a card's chip public key and a
//!   price; a buyer pays in a Stellar stablecoin (USDC on testnet) and the
//!   token is minted to them atomically.
//! * **Authenticity** — anyone holding the physical card can have the chip sign
//!   a fresh challenge; [`RecordPoolCard::authenticate`] verifies it on-chain
//!   with the native `secp256r1_verify` host function and returns the owner.
//! * **Secondary sale with enforced royalties** — owners list a card, buyers
//!   pay through the contract, and the royalty from the OpenZeppelin
//!   `Royalties` extension is split out in the same transaction.
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
pub use storage::{Config, Listing};

#[cfg(test)]
mod test;
