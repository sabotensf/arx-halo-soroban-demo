# From ERC-721 to Soroban: a field guide

*For teams that have shipped NFTs (or ERC-5791 Physical Backed Tokens) on Ethereum and are bringing a physical-backed product to Stellar.
Every Soroban snippet comes from this repository's [`RecordPoolSmartCard`](../contracts/recordpool-smart-card/src/contract.rs) contract.*

---

## 1. Assembling the token

**Solidity (OpenZeppelin 5.x)**: the contract inherits from several base contracts. When two of them define the same function, Solidity makes you override it and name both parents, even if the override only calls `super`. With `ERC721Enumerable` and `AccessControl`, that's three functions of boilerplate:

```solidity
contract RecordPoolSmartCard is
    ERC721,
    ERC721Enumerable,
    AccessControl
{
    bytes32 public constant ISSUER = keccak256("ISSUER");

    function _update(address to, uint256 id, address auth)
        internal
        override(ERC721, ERC721Enumerable)
        returns (address)
    {
        return super._update(to, id, auth);
    }

    function _increaseBalance(address account, uint128 amount)
        internal
        override(ERC721, ERC721Enumerable)
    {
        super._increaseBalance(account, amount);
    }

    function supportsInterface(bytes4 interfaceId)
        public
        view
        override(ERC721, ERC721Enumerable, AccessControl)
        returns (bool)
    {
        return super.supportsInterface(interfaceId);
    }

    // ...plus the PBT functions
}
```

**Soroban (OpenZeppelin Stellar 0.8)**: each trait you implement gets its default methods exported as contract functions:

```rust
#[contractimpl(contracttrait)]
impl NonFungibleToken for RecordPoolSmartCard {
    type ContractType = Enumerable;

    // The only overrides: transfer and transfer_from always fail,
    // because an ERC-5791 token moves only with its chip.
}

#[contractimpl(contracttrait)]
impl NonFungibleEnumerable for RecordPoolSmartCard {}

#[contractimpl(contracttrait)]
impl AccessControl for RecordPoolSmartCard {}
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
pub fn purchase(
    e: &Env,
    buyer: Address,
    chip_id: BytesN<32>,
    chip_signature: BytesN<64>,
    recovery_id: u32,
    signature_timestamp: u64,
    extras: Bytes,
) -> Purchase {
    // The buyer's wallet: also covers the USDC transfer below.
    buyer.require_auth();
    ...
    // The chip: its signature must name the buyer.
    consume_chip_signature(e, &chip_id, &buyer, &chip_signature, …);

    let config = storage::get_config(e);
    TokenClient::new(e, &config.payment_token).transfer(&buyer, &config.treasury, &price);

    let token_id = <Self as NonFungibleToken>::ContractType::mint(e, &buyer);
    ...
}
```

The test suite checks that the buyer gives exactly one authorization for `purchase`, and that it includes the nested USDC transfer.

Two kinds of authorization sit side by side here. The **wallet** authorizes spending its USDC, through the host's auth tree. The **chip** authorizes the mint, through a signature the contract checks itself. `transfer_token` uses only the second: it calls no `require_auth` at all, so anyone (usually the new holder) can submit the chip's signature and pay the fee.

## 3. Taking stablecoin payments

**Ethereum**: the buyer sends `USDC.approve(card, price)`, waits for it to confirm, then calls `card.purchase()`, which runs `USDC.transferFrom`. That's two transactions, two fees, and an allowance that stays open afterwards. Permit-style signatures (EIP-2612) help, but each token has to support them.

**Stellar**: USDC is a native Stellar asset issued by Circle. Its **Stellar Asset Contract (SAC)** exposes the same balances to Soroban through the standard SEP-41 token interface. The buyer signs once, and the contract moves the funds inside that authorization. The USDC the contract receives is the same USDC a Stellar wallet, exchange or anchor already holds, so there's nothing to bridge or wrap.

One Stellar-specific step: a classic `G…` account needs a **trustline** to hold USDC. `scripts/common.sh` sets one up with `stellar tx new change-trust`.

## 4. Royalties

ERC-5791 doesn't cover royalties, and neither does this contract. On Ethereum, ERC-2981 only *reports* a royalty for marketplaces to honor, and OpenZeppelin's Stellar library has the same thing as its `Royalties` extension. A Physical Backed Token makes either one moot: the token moves when someone taps the card, and a tap carries no price, so there's nothing to take a royalty from.

A paid resale needs a contract in the middle, for example an escrow that releases the buyer's USDC (minus the royalty) to the seller when the chip signs the transfer to the buyer. On Stellar that settlement would still be one buyer signature, because the USDC transfer nests inside the same auth tree.

## 5. Storage has a lifetime

| | Ethereum | Soroban |
|---|---|---|
| Model | Storage slots, paid once, kept forever | Ledger entries with a **TTL** and rent |
| Tiers | — | `instance` (contract config), `persistent` (ownership, chip bindings), `temporary` (approvals) |
| Approvals | Live until revoked | Expire at `live_until_ledger` (temporary storage) |
| What you do | Nothing | Extend TTLs on access (`storage.rs::bump`); archived entries can be restored |

For a PBT this matters: a card's on-chain record has to stay live as long as the physical card exists. The contract extends each card's entries by about 30 days whenever they're touched, and the contract instance on every state change. An operator job can extend dormant ones.

## 6. Physical Backed Tokens: ERC-5791 as the model

RecordPool cards carry **HaLo** NFC chips, which sign with **secp256k1**, the same curve as Ethereum accounts. ERC-5791 pairs each token with a chip and lets only the chip move it. This contract keeps that model and rebuilds every encoding with Stellar conventions, so the same physical chip works on both networks but signs differently for each.

**What the chip signs here.** HaLo key slot 1 can sign any 32-byte digest, so the contract asks it to sign the digest Stellar's [SEP-53](https://github.com/stellar/stellar-protocol/blob/master/ecosystem/sep-0053.md) defines for signed messages. Stellar accounts sign that digest with Ed25519; the chip signs it with secp256k1:

```rust
// SEP-53 digest
let digest = sha256("Stellar Signed Message:\n" || message);

let pubkey = e.crypto().secp256k1_recover(&digest, &rs, recovery_id);

// 32-byte chip id
let chip_id = sha256(&pubkey);
if chip_id != expected {
    panic_with_error!(e, CardError::InvalidSignature);
}
```

**What's different from the ERC-5791 reference:**

| | Ethereum (ERC-5791 reference) | Soroban (this repo) |
|---|---|---|
| Chip id | `address`: last 20 bytes of keccak256 of the public key | `BytesN<32>`: SHA-256 of the public key |
| Signed digest | EIP-191: keccak256 of `"\x19Ethereum Signed Message:\n32" ‖ hash` | SEP-53: SHA-256 of `"Stellar Signed Message:\n" ‖ message` |
| Signature | 65 bytes `r ‖ s ‖ v` | 64 bytes `r ‖ s` plus `recovery_id`, as `secp256k1_recover` takes them |
| Recover the signer | `ecrecover` → 20-byte address, zero on failure | `secp256k1_recover` → 65-byte public key, then SHA-256. Failure is a host error, not a silent zero. |
| Message encoding | `abi.encode(address(this), block.chainid, nonce, to, timestamp, keccak256(extras))` | `pbt:transfer:` tag + SHA-256 over XDR addresses, `network_id` instead of `chainid`, `chip_id` included. A `signature_message` view returns it, so clients don't rebuild XDR. |
| Purpose separation | None: an `isChipSignatureForToken` challenge and a transfer use the same digest | Transfers sign `pbt:transfer:…`, authenticity checks `pbt:verify:…`, so one can't be used as the other |
| Canceling signatures | None | The owner can call `rotate_nonce` |
| Freshness | `block.timestamp` window | Ledger timestamp window (`max_duration_window`) |
| Nonce rotation | XOR with `blockhash(block.number - 1)` | `SHA-256(digest ‖ prng)`: Soroban contracts can't read a block hash |
| Interface detection | ERC-165 `supportsInterface` | The contract spec in the WASM |
| Receiver hooks | `useSafeTransferFrom` flag | Dropped: Soroban NFTs have no receiver hook |

Stellar has no standard for this yet. [docs/sep-draft-physical-backed-tokens.md](sep-draft-physical-backed-tokens.md) proposes one, as an extension of SEP-50 that uses SEP-53 digests. Soroban also has a native `secp256r1_verify`, the curve used by passkeys and phone secure enclaves, so a later revision could support P-256 chips too.

## 7. Deploying and initializing

| | Ethereum | Soroban |
|---|---|---|
| Init | Constructor; with proxies, an `initialize()` that can be front-run | `__constructor`, run atomically with deploy |
| Upgrades | Proxy patterns (Transparent, UUPS, Beacon) | Native: a contract can replace its own WASM (`update_current_contract_wasm`); OZ ships an `Upgradeable` helper. Not enabled here. |
| Code reuse | Each deploy stores its bytecode again | WASM is uploaded once (by hash); many instances can share it |

```bash
stellar contract deploy --wasm recordpool_smart_card.wasm --network testnet --source-account rp-admin \
  -- --admin G… --payment_token C…USDC_SAC --treasury G… --max_duration_window 600 \
     --base_uri http://localhost:8000/meta/ --name "RecordPool Smart Card" --symbol RPCARD
```

## 8. Testing

Soroban contracts are ordinary Rust crates. `cargo test` runs the contract in an in-process `Env` that includes a **real Stellar Asset Contract**, so the USDC flows in the tests are the same code that runs on-chain:

```rust
let sac = e.register_stellar_asset_contract_v2(issuer);
StellarAssetClient::new(&e, &sac.address()).mint(&buyer, &(100 * USDC));
```

The suite in [`test.rs`](../contracts/recordpool-smart-card/src/test.rs) covers access control, atomic rollback when a buyer is short of funds, counterfeit chips, replayed signatures, signatures naming the wrong recipient or the wrong contract, the timestamp window, wallet transfers being refused, the music URL unlocking only once a card is sold, and state changes extending the contract instance's TTL.

## Cheat sheet

| ERC-721 / Solidity | Soroban / OZ Stellar |
|---|---|
| `uint256 tokenId` | `u32 token_id` |
| `ownerOf`, `balanceOf` | `owner_of`, `balance` |
| `safeTransferFrom` + `onERC721Received` | `transfer` / `transfer_from` (no receiver hook; disabled here for PBT) |
| ERC-5791 `transferToken(to, chipId, sig, timestamp, safe, extras)` | `transfer_token(to, chip_id, chip_signature, recovery_id, signature_timestamp, extras)` |
| ERC-5791 `tokenIdFor`, `isChipSignatureForToken` | `token_id_for_chip`, `is_chip_signature_for_token` |
| `ecrecover(digest, v, r, s)` | `secp256k1_recover(digest, r‖s, recovery_id)` (recovery_id = v − 27) |
| EIP-191 `personal_sign` | SEP-53 signed messages |
| `setApprovalForAll(op, true)` | `approve_for_all(owner, op, live_until_ledger)` |
| `ERC721Enumerable.tokenOfOwnerByIndex` | `get_owner_token_id(owner, index)` |
| `ERC2981.royaltyInfo` | `royalty_info(token_id, sale_price)` (OZ `Royalties` extension; not used here) |
| `AccessControl.grantRole` | `grant_role(account, role, caller)` |
| `event Transfer(indexed …)` | `#[contractevent] struct Transfer { #[topic] … }` |
| `revert CustomError()` | `panic_with_error!(e, CardError::…)` (`#[contracterror]`) |
| Hardhat / Foundry | `cargo test` + `soroban-sdk` testutils |
| `forge create` / `hardhat deploy` | `stellar contract deploy -- <constructor args>` |
