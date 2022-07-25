# Configure CKB

## How CKB Locates Config File

CKB looks for configuration files in `<config-dir>`, which is the current working directory by default. Different subcommands use different config file names:

-   `ckb run`: `ckb.toml`
-   `ckb miner`: `ckb-miner.toml`
-   `ckb import`: `ckb.toml`
-   `ckb export`: `ckb.toml`

Command line argument `-C <path>` sets the value of `<config-dir>` to `<path>`.

Command `ckb init` initializes a directory by exporting the config files.

Some config file may refer to other files, for example, `chain.spec` in
`ckb.toml` and `system_cells` in chain spec file. The file is referred via
either absolute path, or a path relative to the directory containing the
config file currently being parsed.

Take the following directory hierarchy as an example:

```
ckb.toml
specs/dev.toml
specs/cells/secp256k1_sighash_all
```

Then `ckb.toml` refers `dev.toml` as `specs/dev.toml`, while
`specs/dev.toml` refers `secp256k1_sighash_all` as `cells/secp256k1_sighash_all`.

## How to Change Config

First export the bundled config files into current directory using subcommand `init`.

```
ckb init
```

Then edit the generated config files according to the in-line comments.

## Chain Spec

The option `chain.spec` configures the chain spec, which controls which kind of chain to run.
This option is set to Mirana, the mainnet by default.

The subcommand `init` supports exporting the default options for different
chains. The following command lists all supported chains.

```
ckb init --list-chains
```

Here is an example to export config files for Pudge, the testnet.

```
ckb init --chain testnet
```

Nodes running different chain specs cannot synchronize with each other, so be careful when editing this option.

The dev chain reads the chain spec from file `specs/dev.toml`, developers can edit to switch between different PoW engines.

CKB now supports the following PoW Engines.

### Eaglesong

```
[pow]
func = "Eaglesong"
```

### Eaglesong with an extra Blake2b Hash

Used for testnet.

```
[pow]
func = "EaglesongBlake2b"
```

and the miner workers section in `ckb-miner.toml` should be:

```
[[miner.workers]]
worker_type = "EaglesongSimple"
threads     = 1
extra_hash_function = "Blake2b"
```

### Dummy

```
[pow]
func = "Dummy"
```

and don't forget to modify `ckb-miner.toml` miner workers section:

```
[[miner.workers]]
worker_type = "Dummy"
delay_type  = "Constant"
value       = 5000
```

## How to Run Multiple Nodes

Each node requires its own `<config-dir>`. Since the default ports will conflict, please export the config files and edit the listen ports in the config files.

The option `--genesis-message` is required to set to the same message for dev chain, because by default dev chain generates a random genesis message. Nodes with different genesis messages cannot connect to each other.

```
mkdir node1 node2
ckb -C node1 init --chain dev --genesis-message dev-genesis
ckb -C node2 init --chain dev --genesis-message dev-genesis
# Change listen ports 8114/8115 to 8116/8117 in node2/ckb.toml.
# Change `rpc_url` in node2/ckb.toml to use 8116.
# start node1
ckb -C node1 run
# If you want node2 connects node1, copy the P2P address of node1 in its log.
# Add the address into the section `bootnodes` in `node2/ckb.toml`.
# start node2
ckb -C node2 run
```
