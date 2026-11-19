#!/usr/bin/env bash
# Builds the contract and deploys it to Stellar testnet, paid in Circle testnet USDC.
source "$(dirname "$0")/common.sh"

MAX_WINDOW="${MAX_WINDOW:-600}"      # seconds a chip signature stays valid
BASE_URI="${BASE_URI:-http://localhost:8000/meta/}"   # token_uri = base + id, served from web/meta/

step "Build"
(cd "$ROOT" && stellar contract build --package recordpool-smart-card >/dev/null)
stellar contract optimize --wasm "$WASM" --wasm-out "$WASM" >/dev/null 2>&1 || true
ls -l "$WASM"

step "Identities"
ensure_key "$ADMIN"
ADMIN_ADDR="$(stellar keys address "$ADMIN")"
echo "admin/issuer/treasury: $ADMIN_ADDR"

step "USDC Stellar Asset Contract"
# The SAC for a classic asset only needs deploying once per network; ignore "already exists".
if ! out="$(stellar contract asset deploy --asset "$USDC_ASSET" --source-account "$ADMIN" --network "$NETWORK" 2>&1)"; then
  grep -qi "already exists" <<<"$out" || { echo "$out" >&2; exit 1; }
fi
USDC_SAC="$(usdc_sac)"
echo "USDC SAC: $USDC_SAC"
# The treasury is a classic account, so it needs a USDC trustline to receive proceeds.
stellar tx new change-trust --source-account "$ADMIN" --line "$USDC_ASSET" --network "$NETWORK" >/dev/null

step "Deploy RecordPoolSmartCard"
CONTRACT_ID="$(stellar contract deploy --wasm "$WASM" --source-account "$ADMIN" --network "$NETWORK" --alias recordpool-smart-card \
  -- --admin "$ADMIN_ADDR" --payment_token "$USDC_SAC" --treasury "$ADMIN_ADDR" \
     --max_duration_window "$MAX_WINDOW" --base_uri "$BASE_URI" --name "RecordPool Smart Card" --symbol RPCARD)"
WASM_HASH="$(stellar contract upload --wasm "$WASM" --source-account "$ADMIN" --network "$NETWORK" 2>/dev/null)"

cat > "$DEPLOYMENT_FILE" <<JSON
{
  "network": "$NETWORK",
  "contract_id": "$CONTRACT_ID",
  "wasm_hash": "$WASM_HASH",
  "admin": "$ADMIN_ADDR",
  "treasury": "$ADMIN_ADDR",
  "payment_token": "$USDC_SAC",
  "payment_asset": "$USDC_ASSET",
  "max_duration_window": $MAX_WINDOW,
  "deployed_at": "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
}
JSON
cat "$DEPLOYMENT_FILE"
echo "Explorer: https://stellar.expert/explorer/$NETWORK/contract/$CONTRACT_ID"
