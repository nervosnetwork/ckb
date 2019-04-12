# Quick Start

Following steps will assume that the shell can find the executable `ckb`, see
how to [get CKB](get-ckb.md).

## Start Node

```shell
ckb run
```

It will start a node using the default configurations and store files in `data/dev` in current directory. If you want to customize the configurations, see how to [configure CKB](configure.md).

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
