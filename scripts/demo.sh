#!/usr/bin/env bash
# End-to-end lifecycle of one RecordPool chipped card on testnet:
#   manufacture -> register -> buy with USDC -> tap to authenticate -> resell with royalty
source "$(dirname "$0")/common.sh"

ALICE="${ALICE:-rp-alice}"
BOB="${BOB:-rp-bob}"
PRICE=50000000          # 5 USDC (7 decimals)
RESALE=80000000         # 8 USDC
CHIP_SIM="$ROOT/target/release/chip-sim"
WORK="$(mktemp -d)"; trap 'rm -rf "$WORK"' EXIT

[ -f "$DEPLOYMENT_FILE" ] || { echo "run scripts/deploy-testnet.sh first"; exit 1; }
(cd "$ROOT" && cargo build --release -p chip-sim -q)

step "Identities + testnet USDC (swapped from friendbot XLM on the DEX)"
for k in "$ALICE" "$BOB"; do ensure_key "$k"; done
fund_usdc "$ALICE" 60000000    # 6 USDC
fund_usdc "$BOB"   100000000   # 10 USDC
echo "alice USDC: $(usdc_balance "$ALICE")   bob USDC: $(usdc_balance "$BOB")"

step "1. Manufacture: the card's secure element generates a P-256 key pair"
PUBKEY="$("$CHIP_SIM" keygen "$WORK/card.key")"
echo "chip pubkey: $PUBKEY"

step "2. Issuer registers the card for sale at 5 USDC"
CHIP_ID="$(invoke "$ADMIN" register_card --chip_pubkey "$PUBKEY" --price "$PRICE" --operator "$(stellar keys address "$ADMIN")" | tr -d '"')"
echo "chip_id: $CHIP_ID"

step "3. Alice buys it — USDC payment and mint in one atomic transaction"
TOKEN_ID="$(invoke "$ALICE" purchase --buyer "$(stellar keys address "$ALICE")" --chip_id "$CHIP_ID")"
echo "token_id: $TOKEN_ID   owner: $(view owner_of --token_id "$TOKEN_ID")"

step "4. Tap to verify: chip signs a fresh challenge, contract checks it (read-only)"
CHALLENGE="$("$CHIP_SIM" challenge)"
SIG="$("$CHIP_SIM" sign "$WORK/card.key" "$CHALLENGE")"
echo "authenticate -> owner $(view authenticate --token_id "$TOKEN_ID" --challenge "$CHALLENGE" --signature "$SIG")"
"$CHIP_SIM" keygen "$WORK/fake.key" >/dev/null
FAKE_SIG="$("$CHIP_SIM" sign "$WORK/fake.key" "$CHALLENGE")"
if view authenticate --token_id "$TOKEN_ID" --challenge "$CHALLENGE" --signature "$FAKE_SIG" >/dev/null 2>&1; then
  echo "counterfeit accepted?!"; exit 1
else
  echo "counterfeit chip -> rejected ✓"
fi

step "5. Alice lists the card for 8 USDC; Bob buys it (10% royalty enforced)"
invoke "$ALICE" list --seller "$(stellar keys address "$ALICE")" --token_id "$TOKEN_ID" --price "$RESALE" >/dev/null
TREASURY_BEFORE="$(usdc_balance "$ADMIN")"
ALICE_BEFORE="$(usdc_balance "$ALICE")"
invoke "$BOB" buy --buyer "$(stellar keys address "$BOB")" --token_id "$TOKEN_ID" >/dev/null
echo "owner now: $(view owner_of --token_id "$TOKEN_ID")  (bob = $(stellar keys address "$BOB"))"
python3 - "$TREASURY_BEFORE" "$(usdc_balance "$ADMIN")" "$ALICE_BEFORE" "$(usdc_balance "$ALICE")" <<'PY'
import sys
tb, ta, ab, aa = (int(x.strip('"')) for x in sys.argv[1:])
print(f"royalty to treasury: {(ta-tb)/1e7:.2f} USDC   seller proceeds: {(aa-ab)/1e7:.2f} USDC")
PY

step "Done"
echo "https://stellar.expert/explorer/$NETWORK/contract/$(contract_id)"
