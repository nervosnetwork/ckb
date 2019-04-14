# Quick Start

Following steps will assume that the shell can find the executable `ckb`, see
how to [get CKB](get-ckb.md).

First creates a directory to run CKB

```shell
mkdir ckb-dev
cd ckb-dev
```

All the following commands will run in this same directory.

Then init the directory with the default config files.

```shell
ckb init
```

See how to [configure CKB](configure.md) if you like to tweak the options.

## Start Node

Start the node from the directory

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
