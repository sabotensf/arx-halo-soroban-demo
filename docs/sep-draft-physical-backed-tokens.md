## Preamble

```
SEP: To be assigned
Title: Physical Backed Tokens (Non-Fungible Token Extension)
Author: David Geddes
Track: Standard
Status: Draft (not yet submitted)
Created: 2026-11-16
Updated: 2026-11-19
Version: 0.2.0
Discussion: To be opened
Requires: SEP-50, SEP-53
```

> This is a working draft kept alongside its reference implementation. It has not been submitted to [stellar/stellar-protocol](https://github.com/stellar/stellar-protocol) and has no official status.

## Simple Summary

A standard interface for non-fungible tokens that are paired with a chip embedded in a physical item, where the chip, not a wallet, authorises minting and every transfer. It takes Ethereum's [ERC-5791](https://eips.ethereum.org/EIPS/eip-5791) as its model and specifies it with Stellar conventions, as an extension of [SEP-50](https://github.com/stellar/stellar-protocol/blob/master/ecosystem/sep-0050.md) non-fungible tokens, with chip signatures over [SEP-53](https://github.com/stellar/stellar-protocol/blob/master/ecosystem/sep-0053.md) message digests.

## Dependencies

- [SEP-50](https://github.com/stellar/stellar-protocol/blob/master/ecosystem/sep-0050.md) (Non-Fungible Tokens): the base token interface.
- [SEP-53](https://github.com/stellar/stellar-protocol/blob/master/ecosystem/sep-0053.md) (Sign and Verify Messages): the message prefix and digest that chips sign.
- Soroban host functions `secp256k1_recover` and `sha256`.

## Motivation

NFC chips with secure, non-extractable keys (such as Arx HaLo) let a physical item prove it is present by signing a message when tapped. Pairing such a chip with a token gives physical goods (collectibles, merchandise, tickets, luxury items, music cards) an on-chain record whose ownership **follows the item**: hand someone the item, they tap it, and the token is theirs.

Ethereum standardised this pattern as ERC-5791 ("Physical Backed Tokens", PBT). Stellar's NFT standard (SEP-50) has no physical-backed extension, and SEP-39 covers only classic-asset NFT conventions. Without a shared interface, every Soroban project that binds tokens to chips invents its own message format, which prevents wallets, marketplaces and chip tooling from supporting them generically.

This SEP defines that interface. It keeps ERC-5791's model (chip-authorised transfers, disabled wallet transfers, signatures bound to recipient, nonce and time) and specifies every encoding the Stellar way: SHA-256 hashing, SEP-53 message digests, XDR addresses and the network ID.

## Abstract

A Physical Backed Token (PBT) contract:

- pairs each token with exactly one chip, identified by a 32-byte `chip_id`;
- moves a token only through `transfer_token`, authorised by a fresh chip signature naming the recipient;
- disables the SEP-50 `transfer` and `transfer_from` functions;
- binds every chip signature to the contract, the network, a per-chip nonce, the recipient, a timestamp and optional extra data, so a signature cannot be replayed across contracts, networks or uses;
- lets anyone verify a chip signature over arbitrary data for a given token.

## Specification

### Chip identity

`chip_id` is the SHA-256 hash of the chip's uncompressed SEC1 secp256k1 public key (65 bytes, starting `0x04`):

```
chip_id = SHA-256(uncompressed_public_key)
```

A contract derives it from the key that `secp256k1_recover` returns, so it never needs the key itself in storage.

### Chip signatures

A chip signs the SEP-53 digest of a message with its secp256k1 key:

```
digest = SHA-256("Stellar Signed Message:\n" ‖ message)
```

This is the digest SEP-53 defines for Stellar account keys, which sign it with Ed25519. Chips such as HaLo can't produce Ed25519 signatures, so this SEP keeps SEP-53's prefix and hash and replaces only the signature algorithm.

A chip signature is passed to a contract as two values, matching the `secp256k1_recover` host function:

- `chip_signature`: 64 bytes, `r ‖ s`, each 32 bytes big-endian;
- `recovery_id`: `0` or `1`. Signers that report Ethereum-style `v` (27 or 28) convert with `recovery_id = v − 27`.

A `recovery_id` other than 0 or 1 MUST be rejected.

### Purpose tags

A chip key signs two kinds of message: transfer messages, which move a token, and challenges, which only prove a card is genuine. Each starts with its own tag, so a signature made for one purpose never verifies for the other:

| Purpose | Signed message starts with |
|---|---|
| Mint or transfer | `pbt:transfer:` (13 bytes, ASCII) |
| Authenticity check | `pbt:verify:` (11 bytes, ASCII) |

Without the tags, a page asking the holder to "verify" their card could present a transfer message as its challenge, and the tap would authorise the transfer.

### Transfer message

The message a chip signs to authorise a mint or transfer to `to` is:

```
message = "pbt:transfer:"
        ‖ SHA-256( contract_xdr
                 ‖ network_id
                 ‖ chip_id
                 ‖ chip_nonce
                 ‖ to_xdr
                 ‖ signature_timestamp
                 ‖ SHA-256(extras) )
```

| Field | Encoding |
|---|---|
| `contract_xdr` | XDR of the contract's address as an `ScVal` (`Address::to_xdr`) |
| `network_id` | 32 bytes, SHA-256 of the network passphrase (`env.ledger().network_id()`) |
| `chip_id` | 32 bytes |
| `chip_nonce` | 32 bytes, the chip's current nonce |
| `to_xdr` | XDR of the recipient's address as an `ScVal` |
| `signature_timestamp` | 8 bytes, big-endian `u64` seconds |
| `SHA-256(extras)` | 32 bytes |

The message is 45 bytes: the tag and a 32-byte hash. The chip signs `SHA-256("Stellar Signed Message:\n" ‖ message)`. Contracts MUST expose `message` as a view (`signature_message`) so that clients never have to reproduce the XDR encoding.

### Freshness and replay protection

When consuming a chip signature, a contract MUST:

1. reject it if `signature_timestamp` is later than `env.ledger().timestamp()`;
2. reject it if `signature_timestamp + max_duration_window` is earlier than `env.ledger().timestamp()`, where `max_duration_window` is set by the contract;
3. after a successful check, replace the chip's nonce with a value that depends on the consumed digest and SHOULD be unpredictable before the transaction executes. The reference implementation uses `SHA-256(digest ‖ env.prng().gen::<BytesN<32>>())`.

A chip's nonce starts at 32 zero bytes.

Whoever holds the card can have the chip sign a transfer whose timestamp is in the future, and submit it once that time arrives. Contracts SHOULD let the token's owner cancel all unused signatures by changing the nonce (`rotate_nonce` below).

### Interface

```rust
pub trait PhysicalBackedToken: NonFungibleToken /* SEP-50 */ {
    /// Token paired with `chip_id`. MUST fail if no minted token is paired with it.
    fn token_id_for_chip(e: &Env, chip_id: BytesN<32>) -> u32;

    /// True if the paired chip signed SHA-256("Stellar Signed Message:\n" ‖ data).
    /// MUST fail if `token_id` has no chip, or if `data` doesn't start with
    /// `pbt:verify:`.
    fn is_chip_signature_for_token(
        e: &Env,
        token_id: u32,
        data: Bytes,
        chip_signature: BytesN<64>,
        recovery_id: u32,
    ) -> bool;

    /// Moves the token paired with `chip_id` from its current owner to `to`,
    /// authorised only by a chip signature over `signature_message(to, chip_id,
    /// signature_timestamp, extras)`. MUST NOT require any wallet authorisation.
    /// MUST emit the SEP-50 `transfer` event.
    fn transfer_token(
        e: &Env,
        to: Address,
        chip_id: BytesN<32>,
        chip_signature: BytesN<64>,
        recovery_id: u32,
        signature_timestamp: u64,
        extras: Bytes,
    ) -> u32;

    /// The 45-byte message (`pbt:transfer:` ‖ a 32-byte hash) a chip must sign
    /// (via its SEP-53 digest) for a mint or transfer to `to`.
    fn signature_message(
        e: &Env,
        to: Address,
        chip_id: BytesN<32>,
        signature_timestamp: u64,
        extras: Bytes,
    ) -> Bytes;

    /// The nonce the chip's next signature must cover.
    fn chip_nonce(e: &Env, chip_id: BytesN<32>) -> BytesN<32>;
}

/// OPTIONAL, RECOMMENDED: lets the owner cancel unused chip signatures.
pub trait PhysicalBackedTokenNonceRotation: PhysicalBackedToken {
    /// Replaces the chip's nonce, so every signature made but not yet used
    /// stops verifying. MUST require the authorisation of the token's owner.
    fn rotate_nonce(e: &Env, chip_id: BytesN<32>);
}
```

The SEP-50 `transfer` and `transfer_from` functions MUST fail. `approve` and `approve_for_all` MAY remain but have no effect on transfers.

### Minting

Minting is implementation-defined, as in ERC-5791, with one requirement: a token MUST NOT be minted unless it is paired with a chip, and the recipient SHOULD be authorised by a chip signature over the transfer message (with `to` set to the recipient). A minting function MAY collect payment in the same transaction.

### Events

```rust
/// A chip was paired with a token. Topics: ["chip_set", token_id, chip_id].
#[contractevent]
pub struct ChipSet {
    #[topic] pub token_id: u32,
    #[topic] pub chip_id: BytesN<32>,
}
```

Transfers emit the SEP-50 `transfer` event and mints the SEP-50 `mint` event.

## Design Rationale

ERC-5791 is the model for the behaviour; every encoding follows Stellar conventions instead.

| ERC-5791 | This SEP | Why |
|---|---|---|
| `address chipId`: last 20 bytes of keccak256 of the public key | `BytesN<32>`: SHA-256 of the public key | SHA-256 is the hash Stellar uses throughout; there's no need for an Ethereum address format |
| EIP-191 `personal_sign` digest (keccak256, `"\x19Ethereum Signed Message:\n"`) | SEP-53 digest (SHA-256, `"Stellar Signed Message:\n"`) | Reuses Stellar's own message-signing standard; only the curve differs, because chips can't sign Ed25519 |
| 65-byte signature `r ‖ s ‖ v` | 64-byte `r ‖ s` and a `recovery_id` | Matches the `secp256k1_recover` host function |
| `abi.encode(address(this), block.chainid, …)`, keccak256 | XDR addresses, `network_id`, SHA-256 | Native Soroban encodings; `network_id` plays the role of `chainid` |
| Chip address not in the reference digest | `chip_id` included | ERC-5791's text says the chip address is signed; including it removes any ambiguity between chips |
| Nonce mixed with `blockhash(block.number - 1)` | Nonce mixed with the Soroban PRNG | Soroban contracts can't read a block hash |
| `block.timestamp` window | Ledger timestamp window | Direct equivalent |
| `useSafeTransferFrom` flag | Removed | SEP-50 has no receiver hook to opt into |
| Signature verification via `SignatureCheckerLib` (EOA or ERC-1271) | `secp256k1_recover` only | Covers HaLo and other secp256k1 chips; chips on other curves (e.g. secp256r1 via `secp256r1_verify`) are left to a future revision |
| Client builds the digest | `signature_message` view | Avoids reimplementing XDR encoding in every client |
| Optional ERC-165 `supportsInterface` | Not included | Soroban contracts publish their interface in the contract spec |

A consequence of following Stellar conventions is that a PBT on Stellar is not signature-compatible with an ERC-5791 token: the same physical chip works with both, but a signature made for one won't verify on the other, and the chip's id differs. Since the two networks can't share token state anyway, this costs little, and it keeps Stellar's standards internally consistent.

## Security Concerns

- **Chip authenticity.** The standard trusts whoever pairs chips with tokens. A software key paired as a "chip" would let its holder move the token freely. Implementations SHOULD verify a manufacturer attestation (for HaLo, `attestSig`) before pairing.
- **Raw-digest signing.** Chips that sign arbitrary digests (such as HaLo key slot 1) will sign whatever a client sends them. The SEP-53 prefix keeps a PBT signature from being mistaken for one made for a different scheme, the purpose tags keep an authenticity check from being used as a transfer, and the transfer message binds it to one contract, network, nonce and recipient. Wallet and app developers should still only send digests they built themselves, and SHOULD refuse to sign a `pbt:transfer:` message outside a transfer the user started.
- **Pre-signed transfers.** A chip's nonce is public, so whoever holds the card can have it sign a transfer for later. The owner can cancel such signatures with `rotate_nonce`.
- **Possession, not consent.** Whoever can tap the chip can move the token. That is the point of the standard, but it means a lost or stolen item takes its token with it, as with any bearer asset.
- **Signature lifetime.** A signature is valid for `max_duration_window`. Short windows limit the use of a signature captured during a tap; very short windows can reject legitimate transactions when ledger time lags the client's clock.
- **Front-running.** A signature names its recipient, so a third party who sees it can only submit it for that recipient.
- **Stateless verification.** `is_chip_signature_for_token` proves the chip signed `data`, not when. Callers SHOULD put a fresh random value after the `pbt:verify:` tag.
- **Royalties.** Out of scope, as in ERC-5791. A tap transfer has no on-chain price, so a royalty can only be collected if sales are routed through a contract that pays it, such as an escrow released by the chip-signed transfer.

## Reference Implementation

[`contracts/recordpool-smart-card`](../contracts/recordpool-smart-card/src/contract.rs) in this repository, built on OpenZeppelin Stellar Contracts 0.8: a PBT for NFC music cards with USDC primary sales and tap-to-play, deployed to Stellar testnet. The music URL is application data, not part of this standard.

## Changelog

- `v0.2.0`: purpose tags (`pbt:transfer:`, `pbt:verify:`); `signature_message` returns `Bytes`; optional `rotate_nonce`.
- `v0.1.0`: initial draft.
