# Quick Start

## Start Node

Create the default runtime directory:

```shell
cp -r nodes_template/ nodes
```

Use the config file to start the node

```shell
target/release/ckb run
```

It searches config file `ckb.toml`, `nodes/default.toml` in the shell
working directory in that order. Alternatively, the argument `-c` can specify
the config file used to start the node.

The default config file saves data in `nodes/default/`.

## Use RPC

Find RPC port in the log output, the following command assumes 8114 is used:

```shell
curl -d '{"id": 1, "jsonrpc": "2.0", "method":"get_tip_header","params": []}' \
  -H 'content-type:application/json' 'http://localhost:8114'
```

## Run Miner

Run miner, gets a block template to mine.

```shell
target/release/ckb miner
```

## Run Multiple Nodes

Run multiple nodes in different data directories.

Create the config file for new nodes, for example:

```shell
cp nodes/default.toml nodes/node2.toml
```

Update `data_dir` configuration in config file to a different directory.

```
data_dir = "node2"
```

Then start the new node using the new config file

```shell
target/release/ckb run -c nodes/node2.toml
```

The option `ckb.chain` configures the chain spec. It accepts a path to the spec toml file. The directory `nodes_template/spec` has all the pre-defined specs. Please note that nodes with different chain specs may fail to connect with each other.

The chain spec can switch between different PoW engines. Wiki has the [instructions](https://github.com/nervosnetwork/ckb/wiki/PoW-Engines) about how to configure it.
