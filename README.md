# HaLo & NTAG 424 NFC Chips on Stellar Soroban — RWA-backed NFTs for tap-to-play music cards

> **Disclaimer:** This is an independent demo project. It is not affiliated with, endorsed by, or sponsored by Arx Research, NXP Semiconductors, the Stellar Development Foundation, OpenZeppelin or Circle. "HaLo", "Arx", "NTAG" and "NXP" are referenced only to describe the NFC chips this demo is designed to work with.

A **RecordPool card** is an NFC chip card for music: tap it on a phone and it plays.
Each card carries one of two kinds of NFC chip:

- an Arx [HaLo](https://github.com/arx-research/libhalo) chip with its own secp256k1 key pair, which can't be read out or copied, or
- an NXP [NTAG 424 DNA](https://www.nxp.com/products/NTAG424DNA) chip, which proves each tap with an AES-128 MAC and a tap counter.

This Soroban contract gives each card exactly one on-chain token, sells it for **USDC**,
lets anyone holding the card **prove that it's genuine**, and **pays a royalty** when it's resold through its marketplace.
HaLo taps are verified entirely on-chain. NTAG 424 taps are verified off-chain by a trusted `verifier` and attested on-chain, where the contract rejects replays.

It's built on [OpenZeppelin Stellar Contracts](https://github.com/OpenZeppelin/stellar-contracts) v0.8.0.
The token standard, enumeration, royalty bookkeeping and role-based access control come from that audited library.
This repo only adds the RecordPool-specific logic: binding chips to tokens, USDC checkout, chip authentication, tap attestation and resale settlement.

| | |
|---|---|
| **Network** | Stellar testnet |
| **Contract** | [`CAQJZ53EWHFM5EGLSCV65TLZUBN5LHK6S55BFC4PSEYR2VPAHMPHMN2T`](https://stellar.expert/explorer/testnet/contract/CAQJZ53EWHFM5EGLSCV65TLZUBN5LHK6S55BFC4PSEYR2VPAHMPHMN2T) |
| **Payment token** | Circle testnet USDC — SAC [`CBIELTK6…DAMA`](https://stellar.expert/explorer/testnet/contract/CBIELTK6YBZJU5UP2WWQEUCYKLPU6AUNZ2BQ4WWFEIE3USCIHMXQDAMA) (`USDC:GBBD47IF…LFLA5`) |
| **Hardware** | [Arx Research HaLo](https://arx-docs.vercel.app/HaLo/specifications) NFC chips (secp256k1), read with [libhalo](https://github.com/arx-research/libhalo) · [NXP NTAG 424 DNA](https://www.nxp.com/products/NTAG424DNA) chips (AES-128, Secure Dynamic Messaging) |
| **Stack** | Rust · soroban-sdk 28 · OpenZeppelin `stellar-tokens` / `stellar-access` / `stellar-macros` 0.8 · Stellar CLI 30 |

---

## What's a HaLo chip?

HaLo ("Hardware Locked") is an NFC chip made by [Arx Research](https://arx-docs.vercel.app/HaLo/specifications) for linking physical objects to blockchains. Embedded in a card, a sticker or a product, it works like a tiny hardware wallet:

- **The keys live on the chip.** It generates its own secp256k1 key pairs (the curve Bitcoin and Ethereum use). The chip offers no way to extract, replace or delete its default keys. A copied card has no key, so it can't produce valid signatures.
- **It signs when tapped.** Hold it to a phone and it can sign a message, which proves that this specific physical object is present right now.
- **No app needed.** Arx's open-source [libhalo](https://github.com/arx-research/libhalo) library talks to the chip from a phone browser (Chrome on Android, Safari on iOS), from mobile apps and from desktop NFC readers.

In this demo, each RecordPool card's HaLo key is recorded on-chain when the card is registered, and the Soroban contract checks the chip's signatures with a native host function. See [HaLo chips](#halo-chips) below for the exact signing format.

## What's an NTAG 424 DNA chip?

NTAG 424 DNA is NXP's mass-market anti-counterfeiting NFC chip, used on luxury goods, wine and event tickets. Instead of a key pair it holds **secret AES-128 keys**. With Secure Dynamic Messaging (SDM) turned on, every tap makes it emit a fresh URL:

```
https://…/tap?picc_data=<UID + tap counter, AES-encrypted>&cmac=<AES-CMAC over them>
```

Any phone can read it with no app, and the tap counter goes up on every tap. Checking the CMAC needs the chip's secret key, though, and a smart contract can't keep a secret: anything it checks with would be readable on the public ledger. So this contract handles NTAG 424 cards with a **trusted verifier**:

1. A backend with the `verifier` role holds the AES keys. It decrypts `picc_data`, checks the `cmac`, and reads the UID and counter. [`tools/ntag-tool`](tools/ntag-tool/src/main.rs) does this.
2. The verifier calls `attest_tap(chip_id, counter)`. The contract checks that the chip is a sold NTAG 424 card and that **the counter is higher than any attested before**, so a copied tap URL can't be attested twice.

| | HaLo | NTAG 424 DNA |
|---|---|---|
| Keys | secp256k1 key pair, private key stays on the chip | AES-128 secret keys, shared with whoever verifies |
| Who can check a tap | Anyone with the public key, including the contract | Only the holder of the AES key |
| On Soroban | `authenticate`: verified on-chain, no one to trust | `attest_tap`: verified off-chain by the `verifier` role, replay-checked on-chain |
| Replay protection | Fresh random challenge per tap | Tap counter, enforced by the contract |
| Phone support | Plain taps anywhere; signing a challenge needs Web NFC or an app | Any NFC phone: it's just a URL |

The kind of key decides where verification can live: public-key chips can be checked by the chain itself, symmetric-key chips need an oracle.

## Why this use case

Stellar is built for payments, stablecoins and real-world assets. A tap-to-play music card touches all three:

- **RWA**: the token stands for a specific physical card. Each chip is bound to its token on-chain, so the token and the card are cryptographically linked. The NFC tap that plays the music also proves the card is genuine: a HaLo chip signs a challenge, and an NTAG 424 chip emits a MAC'd tap counter.
- **Stablecoin payments**: both the primary sale and resales settle in USDC through its Stellar Asset Contract (SAC). It's the same USDC that classic Stellar accounts, anchors and wallets already hold.
- **Programmable commerce**: the royalty is paid out inside the resale transaction. It isn't left to a marketplace to honour. A royalty can be set per card, so resales of an artist's card can pay that artist.

## Architecture

```mermaid
flowchart LR
    subgraph Physical["Physical world"]
        CHIP["RecordPool card<br/>HaLo chip (secp256k1)<br/>or NTAG 424 DNA (AES)"]
        PHONE["Phone / POS reader<br/>(RecordPool app)"]
        CHIP <-- "NFC tap:<br/>sign_challenge / SUN URL" --> PHONE
    end
    VERIFIER["Tap verifier backend<br/>(verifier role, AES keys)"]
    PHONE -- "NTAG 424 SUN URL" --> VERIFIER

    subgraph Stellar["Stellar network (Soroban)"]
        direction TB
        subgraph RPC["RecordPoolCard contract"]
            direction TB
            APP["RecordPool logic<br/>register_card · register_ntag_card · purchase<br/>authenticate · attest_tap · list · buy"]
            subgraph OZ["OpenZeppelin Stellar Contracts"]
                NFT["NonFungibleToken<br/>Compose&lt;(Enumerable, Royalties)&gt;"]
                ACL["AccessControl<br/>admin · issuer · verifier"]
            end
            APP --> NFT
            APP --> ACL
        end
        USDC["USDC Stellar Asset Contract<br/>(SEP-41 token)"]
        HOST["Soroban host<br/>secp256k1_recover · sha256"]
        APP -- "transfer()" --> USDC
        APP -- "verify chip signature" --> HOST
    end

    ISSUER["RecordPool issuer<br/>(issuer role, treasury)"] -- "register_card(pubkey, price)" --> APP
    PHONE -- "purchase / authenticate / buy<br/>(signed by user wallet)" --> APP
    VERIFIER -- "attest_tap(chip_id, counter)" --> APP
    USDC -- "proceeds + royalties" --> ISSUER
```

### Card lifecycle

```mermaid
sequenceDiagram
    autonumber
    participant F as Factory / Issuer
    participant C as Card chip
    participant A as Alice (wallet)
    participant B as Bob (wallet)
    participant K as RecordPoolCard
    participant U as USDC SAC

    Note over C: HaLo chip ships with a<br/>non-exportable secp256k1 key (pk2)
    F->>C: read pk2 (libhalo get_pkeys)
    F->>K: register_card(pk2, 5 USDC)  [issuer role]
    Note over K: chip_id = sha256(pubkey) → Pending

    A->>K: purchase(alice, chip_id)  [alice signs once]
    K->>U: transfer(alice → treasury, 5 USDC)
    K->>K: OZ Enumerable::mint → token #0, bind chip ↔ token

    A->>C: tap: sign_challenge(fresh random 32 bytes)
    C-->>A: DER signature over sha256(prefix ‖ challenge)
    A->>A: halo-tool prepare → r‖s (low-S) + recovery id
    A->>K: authenticate(#0, challenge, sig, recovery_id)  [simulation, free]
    K-->>A: owner = alice ✓  (counterfeit chip → error)

    A->>K: list(#0, 8 USDC)
    B->>K: buy(bob, #0)  [bob signs once]
    K->>K: OZ royalty_info(#0, 8) → (treasury, 0.8)
    K->>U: transfer(bob → treasury, 0.8 USDC)
    K->>U: transfer(bob → alice, 7.2 USDC)
    K->>K: move #0 alice → bob, clear listing
```

An NTAG 424 card follows the same sale path, but proves a tap through the verifier:

```mermaid
sequenceDiagram
    autonumber
    participant F as Issuer
    participant C as NTAG 424 chip
    participant A as Alice (phone)
    participant V as Verifier (AES keys)
    participant K as RecordPoolCard

    F->>K: register_ntag_card(sha256(UID), 5 USDC)  [issuer role]
    A->>K: purchase(alice, chip_id)  → token #2
    A->>C: tap
    C-->>A: URL ?picc_data=…&cmac=…  (counter = 1)
    A->>V: open URL
    V->>V: decrypt picc_data, check cmac → UID, counter
    V->>K: attest_tap(sha256(UID), 1)  [verifier role]
    K-->>V: owner = alice ✓
    Note over K: same URL again → counter 1 ≤ 1 → StaleTapCounter
```

## Contract interface

| Function | Who | What it does |
|---|---|---|
| `__constructor(admin, payment_token, treasury, royalty_bps, base_uri, name, symbol)` | deployer | Runs atomically with deployment. Sets OZ metadata, the default royalty, the admin and the `issuer` role. |
| `register_card(chip_pubkey, price, operator)` | `issuer` | Adds a HaLo card to inventory and returns its `chip_id` = SHA-256(public key). |
| `register_ntag_card(chip_id, price, operator)` | `issuer` | Adds an NTAG 424 card to inventory. `chip_id` = SHA-256(UID), computed off-chain so the raw UID isn't published. |
| `purchase(buyer, chip_id) → token_id` | buyer | Pays `price` in USDC to the treasury and mints the token, in one atomic transaction. |
| `authenticate(token_id, challenge, signature, recovery_id) → owner` | anyone | HaLo cards: verifies the chip's `sign_challenge` signature on-chain by recovering the signer key. Run it as a free simulation. |
| `attest_tap(chip_id, counter, verifier) → owner` | `verifier` | NTAG 424 cards: records a tap the verifier checked off-chain. Rejects any counter that isn't higher than the last one. |
| `list(seller, token_id, price)` / `cancel_listing` | owner | Creates or cancels a resale listing. |
| `buy(buyer, token_id)` | buyer | Pays the royalty and the seller from one signature, then moves the token. |
| `card_price`, `token_for_chip`, `chip`, `last_tap_counter`, `get_listing`, `config` | anyone | Read-only lookups. `chip` returns `Halo(pubkey)` or `Ntag424`. |
| `set_treasury` | `issuer` | Changes where primary-sale proceeds go. |
| **From OpenZeppelin:** `balance`, `owner_of`, `transfer`, `transfer_from`, `approve`, `approve_for_all`, `name`, `symbol`, `token_uri`, `total_supply`, `get_token_id`, `get_owner_token_id`, `royalty_info`, `set_default_royalty`, `set_token_royalty`, `grant_role`, `revoke_role`, `has_role`, `transfer_admin_role`, … | | |

### What comes from OpenZeppelin vs. what's custom

```rust
// The token standard is assembled from OZ extensions, not hand-written:
#[contractimpl(contracttrait)]
impl NonFungibleToken for RecordPoolCard {
    type ContractType = Compose<(Enumerable, Royalties)>;
    // transfer/transfer_from wrapped only to invalidate stale resale listings
}
#[contractimpl(contracttrait)] impl NonFungibleEnumerable for RecordPoolCard {}
#[contractimpl(contracttrait)] impl NonFungibleRoyalties  for RecordPoolCard { /* #[only_role(operator, "issuer")] */ }
#[contractimpl(contracttrait)] impl AccessControl         for RecordPoolCard {}
```

All the custom code is in [`contracts/recordpool-card/src/contract.rs`](contracts/recordpool-card/src/contract.rs). It's about 250 lines of business logic, and none of it re-implements anything the library already provides.

## Ethereum vs. Soroban

The ERC-721 patterns, and how this project does the same thing on Stellar. A longer, partner-facing version with code side by side is in [docs/ethereum-vs-soroban.md](docs/ethereum-vs-soroban.md).

| Concern | Ethereum (ERC-721 + OZ Solidity) | Soroban (this repo) |
|---|---|---|
| Building the token | Multiple inheritance: `is ERC721, ERC721Enumerable, ERC2981, AccessControl`, plus `_update` / `supportsInterface` overrides to resolve the diamond | Type-level composition: `type ContractType = Compose<(Enumerable, Royalties)>`. Invalid combinations don't compile. |
| Who is calling? | Implicit `msg.sender` | Explicit `address.require_auth()`. Accounts and contracts are both `Address`. |
| Paying with USDC | `approve` tx, then the contract calls `transferFrom`: two transactions plus a standing allowance | The buyer signs **one** authorization tree that covers `purchase` and the nested `USDC.transfer`. No allowance is left behind. |
| Which USDC | An ERC-20 deployment on each chain | The **same** Circle-issued Stellar asset that wallets and anchors hold, exposed to contracts through its SAC |
| Royalties | ERC-2981 is advisory; marketplaces may ignore it | `royalty_info` from OZ, paid out inside `buy()` in the same transaction |
| Approvals | Live until revoked | Kept in temporary storage with a `live_until_ledger` expiry |
| Storage cost | Pay once, stored forever | State has a TTL and rent (instance / persistent / temporary tiers). The contract bumps TTLs on access. |
| Initialization | Constructor, or `initialize()` behind a proxy (front-running risk) | `__constructor` runs atomically with deploy |
| Physical-chip signatures (HaLo, secp256k1) | Native too: `ecrecover` | `secp256k1_recover` host function. Soroban also has `secp256r1_verify` for P-256 chips and passkeys. |
| Symmetric-key chips (NTAG 424 DNA) | Needs an oracle / attester (e.g. an EAS attestation from a backend) | Same trust model: a `verifier` role attests, and the contract enforces the tap counter |
| `safeTransferFrom` receiver hook | `onERC721Received` | No receiver hook (OZ documents this) |
| Testing | Hardhat / Foundry against an EVM | Plain `cargo test` with an in-process `Env`, a real SAC, mocked or asserted auth trees |

## Run it

### Prerequisites

- Rust stable ≥ 1.91 with the `wasm32v1-none` target (pinned in `rust-toolchain.toml`)
- [Stellar CLI](https://developers.stellar.org/docs/tools/cli) ≥ 30

### Test

```bash
cargo test                       # 29 contract tests (OZ + USDC SAC + simulated HaLo and NTAG chips) + 3 halo-tool + 3 ntag-tool tests
cargo clippy --all-targets -- -D warnings
stellar contract build --package recordpool-card
```

### Deploy and run the full lifecycle on testnet

```bash
./scripts/deploy-testnet.sh      # creates/funds rp-admin, deploys, writes deployments/testnet.json
./scripts/demo.sh                # alice + bob: swap XLM→USDC on the testnet DEX, buy, tap, resell; then an NTAG 424 card
```

### HaLo chips

Each card carries a [HaLo](https://github.com/arx-research/libhalo) NFC chip from Arx Research. HaLo keys are secp256k1 and can't be exported. Key slot 2 (`pk2`) is the card's identity. That slot can't sign arbitrary digests, only domain-separated messages such as `sign_challenge`:

```
signature = ECDSA-secp256k1( SHA-256( "\x19External chall pk2:\n" ‖ challenge[32] ) )   → DER
```

The contract recomputes the same digest and calls Soroban's `secp256k1_recover`. Because of the prefix, a signature the chip makes for any other purpose can't be passed off as a card authentication (the test suite checks this).

HaLo returns DER, but Soroban needs 64-byte r‖s with a low S value plus a recovery id. [`tools/halo-tool`](tools/halo-tool/src/main.rs) does that conversion and can also simulate a chip:

```bash
cargo run -p halo-tool -- challenge                                # fresh 32-byte challenge
cargo run -p halo-tool -- prepare <pk2> <challenge> <der-sig>      # → "<signature> <recovery_id>"
cargo run -p halo-tool -- sim-keygen card.key                      # simulated chip (used by demo.sh)
cargo run -p halo-tool -- sim-sign-challenge card.key <challenge>
```

With a real card, the browser side uses libhalo (Web NFC on Android, the OS NFC prompt on iOS):

```js
import { execHaloCmdWeb } from "@arx-research/libhalo/api/web";

const { publicKeys } = await execHaloCmdWeb({ name: "get_pkeys" });                      // pk2 → register_card
const { signature } = await execHaloCmdWeb({ name: "sign_challenge", challenge: hex32 }); // DER → halo-tool prepare
```

### NTAG 424 DNA chips

[`tools/ntag-tool`](tools/ntag-tool/src/main.rs) is the verifier side. It implements NXP's SUN message format (application note AN12196) and is tested against NXP's published example:

```
picc_data = AES-128-CBC( K_SDMMetaRead, IV=0, 0xC7 ‖ UID[7] ‖ counter[3, LSB first] ‖ random[5] )
K_ses     = AES-CMAC( K_SDMFileRead, 3C C3 00 01 00 80 ‖ UID ‖ counter )
cmac      = odd-indexed bytes of AES-CMAC( K_ses, "" )
```

```bash
cargo run -p ntag-tool -- chip-id <uid>                 # chip_id for register_ntag_card
cargo run -p ntag-tool -- sim-tap <uid> <counter>       # simulated tap → "<picc_data> <cmac>" (used by demo.sh)
cargo run -p ntag-tool -- verify <picc_data> <cmac>     # → "<chip_id> <counter>", exit 1 if forged
```

Keys come from `NTAG_META_KEY` / `NTAG_FILE_KEY` and default to all zeros, which is the factory default on a new chip. A real card would have its own keys written at manufacture (with an NFC writer such as NXP TagWriter) and kept in a secrets manager.

### Testnet run (2026-11-16)

| Step | Transaction |
|---|---|
| Deploy (constructor) | [`0242db5e…`](https://stellar.expert/explorer/testnet/tx/0242db5eefec5fee06edd404425eae61d717b3c4d29721f50afe925f207346ba) |
| **HaLo card** | |
| `register_card` — 5 USDC | [`a02119a5…`](https://stellar.expert/explorer/testnet/tx/a02119a55aff1fc07a04d988e5d31e3969a81807a2f31795fde9d0da005b2d5a) |
| `purchase` — USDC transfer + mint, one tx | [`43688efc…`](https://stellar.expert/explorer/testnet/tx/43688efc04b510b5e40faa970648dd8050f8d64a87583b666f91c34154d27e17) |
| `authenticate` — genuine HaLo signature ✓, counterfeit ✗ | simulation (no tx) |
| `list` — 8 USDC | [`dc9c8c1b…`](https://stellar.expert/explorer/testnet/tx/dc9c8c1ba9a3acd61f4a4c99a81dd90d672eecfc3243fbb491c4efa34828c314) |
| `buy` — 0.80 USDC royalty + 7.20 USDC to seller + transfer | [`c54d7d51…`](https://stellar.expert/explorer/testnet/tx/c54d7d518080fc844f4ee5d4905cc354cb3985fc90b52ccf516aa8a9a9f35bad) |
| **NTAG 424 card** | |
| `register_ntag_card` — 5 USDC | [`3d1cd22c…`](https://stellar.expert/explorer/testnet/tx/3d1cd22c4bb395a31ee1cd4d5bd97f47d2e5eed63f9efcfd5972fbeb54b2c85d) |
| `purchase` | [`49bb7152…`](https://stellar.expert/explorer/testnet/tx/49bb7152b4c9e56b74cbdfa04ef4299e7a2a36c17d3e70761e6630d3b1d30aa7) |
| `grant_role` — `verifier` | [`dd9de869…`](https://stellar.expert/explorer/testnet/tx/dd9de869d12b83213fa81cfede48196c0d1f74dd9b6d08f2e50af1ef5192611d) |
| `attest_tap` — counter 1 ✓; same URL replayed ✗ (`StaleTapCounter`); forged cmac ✗ (verifier) | [`f88973eb…`](https://stellar.expert/explorer/testnet/tx/f88973eb183882115dbbc1bb31c1f914c54c273bf577026c6c1b1bf827120108) |

## Design notes and limitations

This is a demonstration project. It has **not been audited**. OpenZeppelin publishes audit reports for the library it builds on.

- **Royalties are enforced only on sales that go through `buy()`.** A plain OZ `transfer` between two wallets can still settle payment off-chain, the same gap ERC-2981 has. A production version could restrict `transfer` (for example, only allowed when the price is zero or to allow-listed marketplaces) at the cost of composability.
- **Freshness of HaLo `authenticate` is up to the app.** It's a stateless, read-only check, so the app has to generate a random challenge for every tap. A stateful version could issue on-chain nonces, or require a chip signature over the buyer's address inside `purchase` to prove possession at checkout.
- **Trust in the chip key starts with the issuer.** `register_card` is gated by the `issuer` role, so whoever holds that role vouches for which keys are real cards. HaLo chips also return a manufacturer attestation for each key (`get_key_info` → `attestSig`), which a production version could check at registration.
- **NTAG 424 taps are only as trustworthy as the `verifier`.** It holds the AES keys and decides which taps are real; the contract can only check the chip type and that counters go up. A compromised verifier can be removed with `revoke_role`. Counters may jump: a tap that never reached the verifier just leaves a gap.
- **`chip_id` hides an NTAG UID only weakly.** It's SHA-256 of a 7-byte UID, and NXP UIDs start with `04`, so the UID could be brute-forced. A production version could use a keyed hash or a random serial instead.
- **The HaLo tap-to-play URL isn't verified on-chain.** A HaLo tap also opens a dynamic URL carrying a counter-based signature (`rnd` / `rndsig`). This demo uses an explicit `sign_challenge` instead, because its format is fully documented in libhalo.
- **Classic Stellar accounts need a USDC trustline** to send or receive USDC. The scripts set this up; a production checkout would handle it in the wallet flow (or use a smart-wallet `C…` address, which doesn't need one).
- **Token IDs are `u32`** (the OZ default). Metadata comes from `base_uri + token_id`.

## Repo layout

```
contracts/recordpool-card/   Soroban contract (lib, storage, events, errors, 29 tests)
tools/halo-tool/             HaLo signature converter + chip simulator
tools/ntag-tool/             NTAG 424 DNA SUN verifier + chip simulator
scripts/                     deploy-testnet.sh, demo.sh, common.sh
deployments/testnet.json     live deployment record
docs/ethereum-vs-soroban.md  partner-facing comparison
```

## License

MIT
