#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
TEST_DIR="$ROOT_DIR/devtools/offckb-dao-test"
CKB_BINARY="${CKB_BINARY:-$ROOT_DIR/target/release/ckb}"
CKB_RPC_URL="${CKB_RPC_URL:-http://127.0.0.1:8114}"
OFFCKB_LOG="${OFFCKB_LOG:-$TEST_DIR/offckb-devnet.log}"

if [ ! -x "$CKB_BINARY" ]; then
  echo "CKB binary is missing or not executable: $CKB_BINARY" >&2
  exit 1
fi

cd "$TEST_DIR"
npm ci

node ./scripts/patch-offckb-devnet.mjs

npx offckb clean >/dev/null 2>&1 || true
npx offckb node --binary-path "$CKB_BINARY" >"$OFFCKB_LOG" 2>&1 &
OFFCKB_PID=$!

cleanup() {
  kill "$OFFCKB_PID" >/dev/null 2>&1 || true
  wait "$OFFCKB_PID" >/dev/null 2>&1 || true
}
trap cleanup EXIT

node ./scripts/wait-rpc.mjs "$CKB_RPC_URL"
npm test
