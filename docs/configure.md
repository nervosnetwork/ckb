# Configure CKB

## How CKB Locates Config File

CKB looks for configuration files in `<config-dir>`, which is the current working directory by default. Different subcommands use different config file names:

-   `ckb run`: `ckb.toml`
-   `ckb miner`: `ckb-miner.toml`
-   `ckb import`: `ckb.toml`
-   `ckb export`: `ckb.toml`
-   `ckb cli`: no config file required yet

Command line argument `-C <path>` sets the value of `<config-dir>` to `<path>`.

Command `ckb init` initializes a directory by exporting the config files.

Some config file may refer to other files, for example, `chain.spec` in
`ckb.toml` and `system_cells` in chain spec file. The file is referred via
either absolute path, or a path relative to the directory containing the
config file currently being parsed. Take following directory hierarchy as an
example:

```
ckb.toml
specs/dev.toml
specs/cells/always_success
```

Then `ckb.toml` refers `dev.toml` as `specs/dev.toml`, while
`specs/dev.toml` refers `always_success` as `cells/always_success`.

For security reason, there is a limitation of the file reference. The bundled
file can only refer to bundled files, while a file located in the file system
can either refer to another file in the file system or a bundled one.

## How to Change Config

First export the bundled config files into current directory using subcommand `init`.

```
ckb init
```

Then edit the generated config files according to the in-line comments.

## Chain Spec

The option `ckb.chain` configures the chain spec, which controls which kind of chain to run.
This option is set to a spec used for development by default.

The subcommand `init` supports exporting the default options for different
chain specs. The following command lists all supported chain specs.

```
ckb init --list-specs
```

Here is an example to export config files for testnet.

```
ckb init --spec testnet
```

Nodes running different chain specs cannot synchronize with each other, so be carefully when editing this option.

## How to Run Multiple Nodes

Each node requires its own `<config-dir>`. Since the default ports will conflict, please export the config files and edit the listen ports in the config files.

```
mkdir node1 node2
ckb -C node1 init
ckb -C node2 init
# Change listen ports 8114/8115 to 8116/8117 in node2/ckb.toml.
# Change `rpc_url` in node2/ckb.toml to use 8116.
# You may also want to add each other as a boot node in the configuration file.
# start node1
ckb -C node1 run
# start node2
ckb -C node2 run
```
