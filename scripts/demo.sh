#!/usr/bin/env bash
# End-to-end lifecycle of one RecordPool card as a Physical Backed Token (modelled on ERC-5791) on testnet:
#   register -> tap + pay USDC to buy -> tap to play -> prove authenticity -> hand the card over -> tap to transfer
source "$(dirname "$0")/common.sh"

ALICE="${ALICE:-rp-alice}"
BOB="${BOB:-rp-bob}"
PRICE=50000000          # 5 USDC (7 decimals)
# The track the card plays once sold. Any public http(s) audio file works.
# The demo track is served locally: python3 -m http.server -d web 8000
MUSIC_URL="${MUSIC_URL:-http://localhost:8000/star-dust.mp3}"
HALO="$ROOT/target/release/halo-tool"
WORK="$(mktemp -d)"; trap 'rm -rf "$WORK"' EXIT

[ -f "$DEPLOYMENT_FILE" ] || { echo "run scripts/deploy-testnet.sh first"; exit 1; }
(cd "$ROOT" && cargo build --release -p halo-tool -q)
addr() { stellar keys address "$1"; }
unquote() { tr -d '"'; }
json() { python3 -c "import json,sys;print(json.load(sys.stdin)['$1'])"; }

# A tap: ask the contract for the message naming <to>, have the chip sign its SEP-53 digest.
# Real card: execHaloCmdWeb({name: "sign", keyNo: 1, digest: <halo-tool digest $msg>})
#            then halo-tool from-halo <signature.raw.r> <signature.raw.s> <signature.raw.v>
# Sets SIG, RID and TS. The timestamp is a little in the past so it's never ahead of the ledger.
tap() {
  local key="$1" to="$2"
  TS=$(( $(date +%s) - 30 ))
  local msg; msg="$(view signature_message --to "$to" --chip_id "$CHIP_ID" --signature_timestamp "$TS" --extras-file-path /dev/null | unquote)"
  read -r SIG RID <<<"$("$HALO" sim-sign "$key" "$msg")"
}

step "Identities + testnet USDC (swapped from friendbot XLM on the DEX)"
for k in "$ALICE" "$BOB"; do ensure_key "$k"; done
fund_usdc "$ALICE" 60000000    # 6 USDC
echo "alice USDC: $(usdc_balance "$ALICE")"

step "1. Card's HaLo chip: chip_id = SHA-256 of its key-slot-1 public key"
# Real card: halo-tool chip-id <libhalo get_pkeys -> publicKeys["1"]>. Here: a simulated chip.
CHIP_ID="$("$HALO" sim-keygen "$WORK/card.key")"
echo "chip_id: $CHIP_ID"

step "2. Issuer registers the card for sale at 5 USDC, with the track it plays"
invoke "$ADMIN" register_card --chip_id "$CHIP_ID" --price "$PRICE" --music_url "$MUSIC_URL" --operator "$(addr "$ADMIN")" >/dev/null
echo "price: $(view card_price --chip_id "$CHIP_ID") (unsold: a tap shows the price, music_url isn't available yet)"

step "3. Alice taps the card and buys it: chip-signed mint + USDC payment, one transaction"
tap "$WORK/card.key" "$(addr "$ALICE")"
BOUGHT="$(invoke "$ALICE" purchase --buyer "$(addr "$ALICE")" --chip_id "$CHIP_ID" \
  --chip_signature "$SIG" --recovery_id "$RID" --signature_timestamp "$TS" --extras-file-path /dev/null)"
TOKEN_ID="$(json token_id <<<"$BOUGHT")"
echo "token_id: $TOKEN_ID   owner: $(view owner_of --token_id "$TOKEN_ID")"
echo "music_url: $(json music_url <<<"$BOUGHT")"

step "4. Anyone taps the card: the tap page looks up the track and plays it (no wallet)"
# A plain tap opens the URL programmed into the chip, with its public key appended as pk1.
# web/index.html derives chip_id from pk1 and simulates music_url, like this:
echo "music_url(chip_id) -> $(view music_url --chip_id "$CHIP_ID")"
echo "tap page: http://localhost:8000/?contract=$(contract_id)&pk1=$("$HALO" sim-pubkey "$WORK/card.key")"

step "5. Is this card genuine? The chip signs a challenge; the contract checks it (read-only)"
# Challenges start with "pbt:verify:", so a verification signature can never move the token.
DATA="$(printf 'pbt:verify:' | od -An -tx1 | tr -d ' \n')$(od -An -N16 -tx1 /dev/urandom | tr -d ' \n')"
check() {
  local sig rid; read -r sig rid <<<"$("$HALO" sim-sign "$1" "$DATA")"
  view is_chip_signature_for_token --token_id "$TOKEN_ID" --data "$DATA" --chip_signature "$sig" --recovery_id "$rid"
}
echo "genuine chip -> $(check "$WORK/card.key")"
"$HALO" sim-keygen "$WORK/fake.key" >/dev/null
echo "other chip   -> $(check "$WORK/fake.key")"

step "6. Wallet transfers are disabled: the card has to move with the chip"
# expect_error <code> <what> <command…>: the command must fail with Error(Contract, #<code>).
expect_error() {
  local code="$1" what="$2" out; shift 2
  if out="$("$@" 2>&1)"; then echo "$what accepted?!"; exit 1; fi
  grep -q "#$code" <<<"$out" || { echo "$what failed for another reason:"; echo "$out"; exit 1; }
}
expect_error 3010 "wallet transfer" invoke "$ALICE" transfer --from "$(addr "$ALICE")" --to "$(addr "$BOB")" --token_id "$TOKEN_ID"
echo "alice -> bob transfer without the chip -> rejected (TransferRequiresChip) ✓"

step "7. Alice hands Bob the card; Bob taps it to take the token"
tap "$WORK/card.key" "$(addr "$BOB")"
invoke "$BOB" transfer_token --to "$(addr "$BOB")" --chip_id "$CHIP_ID" \
  --chip_signature "$SIG" --recovery_id "$RID" --signature_timestamp "$TS" --extras-file-path /dev/null >/dev/null
echo "owner now: $(view owner_of --token_id "$TOKEN_ID")  (bob = $(addr "$BOB"))"
expect_error 3006 "replayed tap" invoke "$BOB" transfer_token --to "$(addr "$BOB")" --chip_id "$CHIP_ID" \
  --chip_signature "$SIG" --recovery_id "$RID" --signature_timestamp "$TS" --extras-file-path /dev/null
echo "same signature replayed -> rejected (nonce changed) ✓"

step "Done"
echo "https://stellar.expert/explorer/$NETWORK/contract/$(contract_id)"
