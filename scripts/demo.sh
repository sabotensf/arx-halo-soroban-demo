#!/usr/bin/env bash
# End-to-end lifecycle of RecordPool chipped cards on testnet:
#   HaLo card:     register -> buy with USDC -> tap, verified on-chain -> resell with royalty
#   NTAG 424 card: register -> buy with USDC -> tap, verified off-chain and attested -> replay rejected
source "$(dirname "$0")/common.sh"

ALICE="${ALICE:-rp-alice}"
BOB="${BOB:-rp-bob}"
VERIFIER="${VERIFIER:-rp-verifier}"
PRICE=50000000          # 5 USDC (7 decimals)
RESALE=80000000         # 8 USDC
HALO="$ROOT/target/release/halo-tool"
NTAG="$ROOT/target/release/ntag-tool"
WORK="$(mktemp -d)"; trap 'rm -rf "$WORK"' EXIT

[ -f "$DEPLOYMENT_FILE" ] || { echo "run scripts/deploy-testnet.sh first"; exit 1; }
(cd "$ROOT" && cargo build --release -p halo-tool -p ntag-tool -q)

step "Identities + testnet USDC (swapped from friendbot XLM on the DEX)"
for k in "$ALICE" "$BOB" "$VERIFIER"; do ensure_key "$k"; done
fund_usdc "$ALICE" 60000000    # 6 USDC
fund_usdc "$BOB"   100000000   # 10 USDC
echo "alice USDC: $(usdc_balance "$ALICE")   bob USDC: $(usdc_balance "$BOB")"

step "1. Card's HaLo chip: read its slot-2 public key (pk2, secp256k1)"
# Real card: libhalo get_pkeys -> publicKeys["2"]. Here: a simulated chip.
PUBKEY="$("$HALO" sim-keygen "$WORK/card.key")"
echo "pk2: $PUBKEY"

step "2. Issuer registers the card for sale at 5 USDC"
CHIP_ID="$(invoke "$ADMIN" register_card --chip_pubkey "$PUBKEY" --price "$PRICE" --operator "$(stellar keys address "$ADMIN")" | tr -d '"')"
echo "chip_id: $CHIP_ID"

step "3. Alice buys it — USDC payment and mint in one atomic transaction"
TOKEN_ID="$(invoke "$ALICE" purchase --buyer "$(stellar keys address "$ALICE")" --chip_id "$CHIP_ID")"
echo "token_id: $TOKEN_ID   owner: $(view owner_of --token_id "$TOKEN_ID")"

step "4. Tap to verify: chip signs a fresh challenge, contract checks it (read-only)"
CHALLENGE="$("$HALO" challenge)"
# Real card: libhalo sign_challenge {challenge} -> DER signature.
DER="$("$HALO" sim-sign-challenge "$WORK/card.key" "$CHALLENGE")"
read -r SIG RID <<<"$("$HALO" prepare "$PUBKEY" "$CHALLENGE" "$DER")"
echo "authenticate -> owner $(view authenticate --token_id "$TOKEN_ID" --challenge "$CHALLENGE" --signature "$SIG" --recovery_id "$RID")"
"$HALO" sim-keygen "$WORK/fake.key" >/dev/null
FAKE_DER="$("$HALO" sim-sign-challenge "$WORK/fake.key" "$CHALLENGE")"
FAKE_PK="$("$HALO" sim-pubkey "$WORK/fake.key")"
read -r FAKE_SIG FAKE_RID <<<"$("$HALO" prepare "$FAKE_PK" "$CHALLENGE" "$FAKE_DER")"
if view authenticate --token_id "$TOKEN_ID" --challenge "$CHALLENGE" --signature "$FAKE_SIG" --recovery_id "$FAKE_RID" >/dev/null 2>&1; then
  echo "counterfeit accepted?!"; exit 1
else
  echo "counterfeit chip -> rejected ✓"
fi

step "5. Alice lists the card for 8 USDC; Bob buys it (10% royalty paid)"
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

step "6. NTAG 424 DNA card: issuer registers it by chip_id = SHA-256(UID)"
# Real card: the UID is read at manufacture, when the SDM keys are written.
CARD_UID="04$(od -An -N6 -tx1 /dev/urandom | tr -d ' \n')"
NTAG_ID="$("$NTAG" chip-id "$CARD_UID")"
invoke "$ADMIN" register_ntag_card --chip_id "$NTAG_ID" --price "$PRICE" --operator "$(stellar keys address "$ADMIN")" >/dev/null
echo "uid: $CARD_UID   chip_id: $NTAG_ID"

step "7. Alice buys the NTAG card; the admin grants the verifier role to the tap backend"
NTAG_TOKEN="$(invoke "$ALICE" purchase --buyer "$(stellar keys address "$ALICE")" --chip_id "$NTAG_ID")"
invoke "$ADMIN" grant_role --account "$(stellar keys address "$VERIFIER")" --role verifier --caller "$(stellar keys address "$ADMIN")" >/dev/null
echo "token_id: $NTAG_TOKEN   verifier: $(stellar keys address "$VERIFIER")"

step "8. Tap: chip emits a SUN URL, the verifier checks its AES-CMAC off-chain and attests on-chain"
read -r PICC MAC <<<"$("$NTAG" sim-tap "$CARD_UID" 1)"
echo "tap URL: https://cards.recordpool.io/tap?picc_data=$PICC&cmac=$MAC"
read -r TAP_ID TAP_CTR <<<"$("$NTAG" verify "$PICC" "$MAC")"
echo "attest_tap -> owner $(invoke "$VERIFIER" attest_tap --chip_id "$TAP_ID" --counter "$TAP_CTR" --verifier "$(stellar keys address "$VERIFIER")")"
if invoke "$VERIFIER" attest_tap --chip_id "$TAP_ID" --counter "$TAP_CTR" --verifier "$(stellar keys address "$VERIFIER")" >/dev/null 2>&1; then
  echo "replayed tap accepted?!"; exit 1
else
  echo "same URL replayed -> rejected on-chain (StaleTapCounter) ✓"
fi
if "$NTAG" verify "$PICC" "0000000000000000" >/dev/null 2>&1; then
  echo "forged tap accepted?!"; exit 1
else
  echo "forged cmac -> rejected by the verifier ✓"
fi

step "Done"
echo "https://stellar.expert/explorer/$NETWORK/contract/$(contract_id)"
