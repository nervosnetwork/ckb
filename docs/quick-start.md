# Quick Start

Following steps will assume that the shell can find the executable `ckb`, see
how to [get CKB](get-ckb.md).

CKB uses current directory to store data. It is recommended to setup the
directory with default config files:

```shell
ckb init
```

See how to [configure CKB](configure.md) if you like to tweak the options.

## Start Node

```shell
ckb run
```

Restarting in the same directory will reuse the data.

## Use RPC

Find RPC port in the log output, the following command assumes 8114 is used:

```shell
curl -d '{"id": 1, "jsonrpc": "2.0", "method":"get_tip_header","params": []}' \
  -H 'content-type:application/json' 'http://localhost:8114'
```

## Run Miner

Run miner, gets a block template to mine.

```shell
ckb miner
```
