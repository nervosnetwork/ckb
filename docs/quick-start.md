# Quick Start

The following steps will assume that the shell can find the executable `ckb`.

First create a directory to run CKB

```shell
mkdir ckb-dev
cd ckb-dev
```

All the following commands will run in the same directory.

Then init the directory with the default config files.

```shell
ckb init
```

See how to [configure CKB](configure.md) if you like to tweak the options.

Windows users can double click `ckb-init-mainnet.bat` to initialize a mainnet
node directory.

## Start Node

Start the node from the directory

```shell
ckb run
```

Restarting in the same directory will reuse the data.

Windows users can double click `ckb-run.bat` to start the node.

## Use RPC

Find RPC port in the log output, the following command assumes 8114 is used:

```shell
curl -d '{"id": 1, "jsonrpc": "2.0", "method":"get_tip_header","params": []}' \
  -H 'content-type:application/json' 'http://localhost:8114'
```

## Run Miner

Miner is disabled by default, unless you have setup the miner lock
to keep your mined CKB safe. See the comment of the section `[block_assembler]`
in `ckb.toml` how to configure it.

After setting up the config file, restart the process `ckb run`, and start the
miner process:

```shell
ckb miner
```
