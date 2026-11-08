#!/usr/bin/env bash
# Shared settings for the testnet scripts.
set -euo pipefail

export NETWORK="${NETWORK:-testnet}"
# Circle's testnet USDC (https://developers.circle.com/stablecoins/usdc-contract-addresses)
export USDC_ASSET="USDC:GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5"
export ADMIN="${ADMIN:-rp-admin}"
export ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export DEPLOYMENT_FILE="$ROOT/deployments/$NETWORK.json"
export WASM="$ROOT/target/wasm32v1-none/release/recordpool_card.wasm"

step() { printf '\n\033[1;36m▶ %s\033[0m\n' "$*"; }

ensure_key() {
  stellar keys address "$1" >/dev/null 2>&1 || stellar keys generate "$1" --network "$NETWORK" --fund
}

usdc_sac() { stellar contract id asset --asset "$USDC_ASSET" --network "$NETWORK"; }

contract_id() { python3 -c "import json;print(json.load(open('$DEPLOYMENT_FILE'))['contract_id'])"; }

# invoke <source> <fn> [args...]
invoke() {
  local src="$1"; shift
  stellar contract invoke --id "$(contract_id)" --source-account "$src" --network "$NETWORK" -- "$@"
}

# view <fn> [args...]  (simulation only, no transaction submitted)
view() {
  stellar contract invoke --id "$(contract_id)" --source-account "$ADMIN" --network "$NETWORK" --send=no -- "$@"
}

usdc_balance() {
  stellar contract invoke --id "$(usdc_sac)" --source-account "$ADMIN" --network "$NETWORK" --send=no \
    -- balance --id "$(stellar keys address "$1")"
}

# Give an account a USDC trustline and buy USDC on the testnet DEX with XLM.
fund_usdc() {
  local who="$1" amount_stroops="$2"
  stellar tx new change-trust --source-account "$who" --line "$USDC_ASSET" --network "$NETWORK" >/dev/null
  stellar tx new path-payment-strict-receive --source-account "$who" --network "$NETWORK" \
    --send-asset native --send-max 2000000000 \
    --destination "$(stellar keys address "$who")" \
    --dest-asset "$USDC_ASSET" --dest-amount "$amount_stroops" >/dev/null
}
