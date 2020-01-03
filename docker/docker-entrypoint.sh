#!/bin/sh

if [ "${1:-}" = "run" ] && ! [ -f ckb.toml ]; then
  /bin/ckb init --chain "$CKB_CHAIN"
fi

exec /bin/ckb "$@"
