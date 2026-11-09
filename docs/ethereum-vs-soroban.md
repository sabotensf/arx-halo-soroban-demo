# From ERC-721 to Soroban: a field guide

*For teams that have shipped NFTs on Ethereum and are bringing a real-world-asset product to Stellar.
Every Soroban snippet comes from this repository's [`RecordPoolCard`](../contracts/recordpool-card/src/contract.rs) contract.*

---

## 1. Assembling the token

**Solidity (OpenZeppelin 5.x)**: multiple inheritance, with hooks you resolve by hand:

```solidity
contract RecordPoolCard is ERC721, ERC721Enumerable, ERC2981, AccessControl {
    bytes32 public constant ISSUER = keccak256("ISSUER");

    function _update(address to, uint256 id, address auth)
        internal override(ERC721, ERC721Enumerable) returns (address)
    { return super._update(to, id, auth); }

    function _increaseBalance(address a, uint128 v)
        internal override(ERC721, ERC721Enumerable)
    { super._increaseBalance(a, v); }

    function supportsInterface(bytes4 id)
        public view override(ERC721, ERC721Enumerable, ERC2981, AccessControl) returns (bool)
    { return super.supportsInterface(id); }
}
```

**Soroban (OpenZeppelin Stellar 0.8)**: you pick one contract type and list the extensions you want. Each trait you implement gets its default methods exported as contract functions:

```rust
#[contractimpl(contracttrait)]
impl NonFungibleToken for RecordPoolCard {
    type ContractType = Compose<(Enumerable, Royalties)>;
}
#[contractimpl(contracttrait)] impl NonFungibleEnumerable for RecordPoolCard {}
#[contractimpl(contracttrait)] impl NonFungibleRoyalties  for RecordPoolCard { /* role-gated setters */ }
#[contractimpl(contracttrait)] impl AccessControl         for RecordPoolCard {}
```

Combinations that don't make sense, such as `Enumerable` together with `Consecutive`, **fail to compile** instead of misbehaving at runtime. There's no `supportsInterface`: clients read the contract spec embedded in the WASM.

## 2. Authorization: `msg.sender` → `require_auth`

| | Ethereum | Soroban |
|---|---|---|
| Identity | `msg.sender` (implicit; `tx.origin` pitfalls) | Any `Address` argument, plus `addr.require_auth()` |
| Accounts vs contracts | EOAs and contracts are different things | Both are `Address` (`G…` accounts, `C…` contracts) |
| Nested calls | Each hop sees a different `msg.sender` | One signed **authorization tree** covers the call and its sub-calls |
| Role gating | `onlyRole(ISSUER)` modifier | `#[only_role(operator, "issuer")]`: checks the role *and* `operator.require_auth()` |

```rust
pub fn purchase(e: &Env, buyer: Address, chip_id: BytesN<32>) -> u32 {
    buyer.require_auth();                              // covers the USDC transfer below too
    ...
    TokenClient::new(e, &config.payment_token).transfer(&buyer, &config.treasury, &card.price);
    let token_id = <Self as NonFungibleToken>::ContractType::mint(e, &buyer);
    ...
}
```

The test suite checks that the buyer gives exactly one authorization for `purchase`, and that it includes the nested USDC transfer.

## 3. Taking stablecoin payments

**Ethereum**: the buyer sends `USDC.approve(card, price)`, waits for it to confirm, then calls `card.purchase()`, which runs `USDC.transferFrom`. That's two transactions, two fees, and an allowance that stays open afterwards. Permit-style signatures (EIP-2612) help, but each token has to support them.

**Stellar**: USDC is a native Stellar asset issued by Circle. Its **Stellar Asset Contract (SAC)** exposes the same balances to Soroban through the standard SEP-41 token interface. The buyer signs once, and the contract moves the funds inside that authorization. The USDC the contract receives is the same USDC a Stellar wallet, exchange or anchor already holds, so there's nothing to bridge or wrap.

One Stellar-specific step: a classic `G…` account needs a **trustline** to hold USDC. `scripts/common.sh` sets one up with `stellar tx new change-trust`.

## 4. Royalties: advertised vs. enforced

ERC-2981 only *reports* a royalty (`royaltyInfo`). Whether it gets paid depends on the marketplace. This contract uses OZ's `NonFungibleRoyalties` for the same bookkeeping (a default royalty plus per-token overrides) and **settles the royalty inside `buy()`**:

```rust
let (royalty_receiver, royalty) =
    <Self as NonFungibleToken>::ContractType::royalty_info(e, token_id, listing.price);
token.transfer(&buyer, &royalty_receiver, &royalty);
token.transfer(&buyer, &seller, &(listing.price - royalty));
// ...then move the NFT with OZ's low-level Base::update + Enumerable bookkeeping
```

To be clear, the same pattern is possible on Ethereum. The difference is that on Stellar the stablecoin is native to the network and one signature covers the whole settlement, which makes "the token contract is the marketplace" a reasonable default. Private `transfer`s still get around it on both chains. See the limitations section in the README.

## 5. Storage has a lifetime

| | Ethereum | Soroban |
|---|---|---|
| Model | Storage slots, paid once, kept forever | Ledger entries with a **TTL** and rent |
| Tiers | — | `instance` (contract config), `persistent` (ownership, chip bindings), `temporary` (approvals) |
| Approvals | Live until revoked | Expire at `live_until_ledger` (temporary storage) |
| What you do | Nothing | Extend TTLs on access (`storage.rs::bump`); archived entries can be restored |

For an RWA this matters: a card's on-chain record has to stay live as long as the physical card exists. The contract extends each card's entries by about 30 days whenever they're touched, and an operator job can extend dormant ones.

## 6. Linking to the physical object

RecordPool cards carry **HaLo** NFC chips, which sign with **secp256k1**, the same curve as Ethereum accounts.

- **Ethereum**: `ecrecover` verifies HaLo signatures natively. This is how HaLo chips are normally used.
- **Soroban**: `e.crypto().secp256k1_recover(...)` does the same job as a host function, so moving to Stellar doesn't mean changing chips. Soroban also has a native `secp256r1_verify`, the curve used by passkeys and most phone secure enclaves, so P-256 hardware would work too.

The contract recomputes the digest a HaLo signs for `sign_challenge`, recovers the signer and compares it with the key bound to the token:

```rust
let mut msg = Bytes::from_slice(e, b"\x19External chall pk2:\n");
msg.append(&challenge.into());
let digest = e.crypto().sha256(&msg);
let signer = e.crypto().secp256k1_recover(&digest, &signature, recovery_id);
if signer != storage::get_chip_pubkey(e, token_id) { panic_with_error!(e, CardError::InvalidChipSignature); }
```

Two practical differences from `ecrecover`:

| | Ethereum `ecrecover` | Soroban `secp256k1_recover` |
|---|---|---|
| Returns | A 20-byte address (the zero address on failure) | The full 65-byte public key. Failure is a host error, not a silent zero. |
| High-S signatures | Accepted by the precompile | **Rejected.** The client has to normalise S to the low half (and flip the recovery id). `halo-tool prepare` does this. |

On both chains `authenticate` is a read-only call, so a wallet or the RecordPool app can run it without paying a fee.

## 7. Deploying and initializing

| | Ethereum | Soroban |
|---|---|---|
| Init | Constructor; with proxies, an `initialize()` that can be front-run | `__constructor`, run atomically with deploy |
| Upgrades | Proxy patterns (Transparent, UUPS, Beacon) | Native: a contract can replace its own WASM (`update_current_contract_wasm`); OZ ships an `Upgradeable` helper. Not enabled here. |
| Code reuse | Each deploy stores its bytecode again | WASM is uploaded once (by hash); many instances can share it |

```bash
stellar contract deploy --wasm recordpool_card.wasm --network testnet --source-account rp-admin \
  -- --admin G… --payment_token C…USDC_SAC --treasury G… --royalty_bps 1000 \
     --base_uri https://cards.recordpool.io/meta/ --name "RecordPool Card" --symbol RPCARD
```

## 8. Testing

Soroban contracts are ordinary Rust crates. `cargo test` runs the contract in an in-process `Env` that includes a **real Stellar Asset Contract**, so the USDC flows in the tests are the same code that runs on-chain:

```rust
let sac = e.register_stellar_asset_contract_v2(issuer);
StellarAssetClient::new(&e, &sac.address()).mint(&buyer, &(100 * USDC));
```

The suite in [`test.rs`](../contracts/recordpool-card/src/test.rs) has 23 tests. They cover access control, atomic rollback when a buyer is short of funds, signatures from counterfeit and replayed chips, exact royalty splits, and listings that a transfer invalidates.

## Cheat sheet

| ERC-721 / Solidity | Soroban / OZ Stellar |
|---|---|
| `uint256 tokenId` | `u32 token_id` |
| `ownerOf`, `balanceOf` | `owner_of`, `balance` |
| `safeTransferFrom` + `onERC721Received` | `transfer` / `transfer_from` (no receiver hook) |
| `setApprovalForAll(op, true)` | `approve_for_all(owner, op, live_until_ledger)` |
| `ERC721Enumerable.tokenOfOwnerByIndex` | `get_owner_token_id(owner, index)` |
| `ERC2981.royaltyInfo` | `royalty_info(token_id, sale_price)` |
| `AccessControl.grantRole` | `grant_role(account, role, caller)` |
| `event Transfer(indexed …)` | `#[contractevent] struct Transfer { #[topic] … }` |
| `revert CustomError()` | `panic_with_error!(e, CardError::…)` (`#[contracterror]`) |
| Hardhat / Foundry | `cargo test` + `soroban-sdk` testutils |
| `forge create` / `hardhat deploy` | `stellar contract deploy -- <constructor args>` |
