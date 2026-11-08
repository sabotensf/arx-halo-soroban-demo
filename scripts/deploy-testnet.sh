#!/usr/bin/env bash
# Builds the contract and deploys it to Stellar testnet, paid in Circle testnet USDC.
source "$(dirname "$0")/common.sh"

ROYALTY_BPS="${ROYALTY_BPS:-1000}"   # 10% default resale royalty
BASE_URI="${BASE_URI:-https://cards.recordpool.io/meta/}"

step "Build"
(cd "$ROOT" && stellar contract build --package recordpool-card >/dev/null)
stellar contract optimize --wasm "$WASM" --wasm-out "$WASM" >/dev/null 2>&1 || true
ls -l "$WASM"

step "Identities"
ensure_key "$ADMIN"
ADMIN_ADDR="$(stellar keys address "$ADMIN")"
echo "admin/issuer/treasury: $ADMIN_ADDR"

step "USDC Stellar Asset Contract"
# The SAC for a classic asset only needs deploying once per network; ignore "already exists".
stellar contract asset deploy --asset "$USDC_ASSET" --source-account "$ADMIN" --network "$NETWORK" >/dev/null 2>&1 || true
USDC_SAC="$(usdc_sac)"
echo "USDC SAC: $USDC_SAC"
# The treasury is a classic account, so it needs a USDC trustline to receive proceeds.
stellar tx new change-trust --source-account "$ADMIN" --line "$USDC_ASSET" --network "$NETWORK" >/dev/null

step "Deploy RecordPoolCard"
CONTRACT_ID="$(stellar contract deploy --wasm "$WASM" --source-account "$ADMIN" --network "$NETWORK" --alias recordpool-card \
  -- --admin "$ADMIN_ADDR" --payment_token "$USDC_SAC" --treasury "$ADMIN_ADDR" \
     --royalty_bps "$ROYALTY_BPS" --base_uri "$BASE_URI" --name "RecordPool Card" --symbol RPCARD)"
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
  "royalty_bps": $ROYALTY_BPS,
  "deployed_at": "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
}
JSON
cat "$DEPLOYMENT_FILE"
echo "Explorer: https://stellar.expert/explorer/$NETWORK/contract/$CONTRACT_ID"
