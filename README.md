<img src="https://raw.githubusercontent.com/poshboytl/tuchuang/master/nervos-logo-dark.png" width="256">

# [Nervos CKB](https://www.nervos.org/) - The Common Knowledge Base

[![TravisCI](https://travis-ci.com/nervosnetwork/ckb.svg?token=y9uR6ygmT3geQaMJ4jpJ&branch=develop)](https://travis-ci.com/nervosnetwork/ckb)
[![dependency status](https://deps.rs/repo/github/nervosnetwork/ckb/status.svg)](https://deps.rs/repo/github/nervosnetwork/ckb)
[![Telegram Group](https://cdn.rawgit.com/Patrolavia/telegram-badge/8fe3382b/chat.svg)](https://t.me/nervos_ckb_dev)

---

## About Nervos CKB

Nervos CKB is the layer 1 of Nervos Network, a public blockchain with PoW and cell model.

Nervos project defines a suite of scalable and interoperable blockchain protocols. Nervos CKB uses those protocols to create a self-evolving distributed network with a novel economic model, data model and more.

## License [![FOSSA Status](https://app.fossa.io/api/projects/git%2Bgithub.com%2Fnervosnetwork%2Fckb.svg?type=shield)](https://app.fossa.io/projects/git%2Bgithub.com%2Fnervosnetwork%2Fckb?ref=badge_shield)

Nervos CKB is released under the terms of the MIT license. See [COPYING](COPYING) for more information or see [https://opensource.org/licenses/MIT](https://opensource.org/licenses/MIT).

## Development Process

This project is still in development, and it's NOT in production-ready status.
The wiki also lists some [known issues](https://github.com/nervosnetwork/ckb/wiki/Known-Issues) that we are currently working on.

The `master` branch is regularly built and tested, however, it is not guaranteed to be completely stable; The `develop` branch is the work branch to merge new features, and it's not stable.

The contribution workflow is described in [CONTRIBUTING.md](CONTRIBUTING.md), and security policy is described in [SECURITY.md](SECURITY.md). To propose new protocol or standard for Nervos, see [Nervos RFC](https://github.com/nervosnetwork/rfcs).

---

## Build dependencies

CKB is currently tested mainly with `stable-1.31.0` on Linux and Mac OSX.

We recommend installing Rust through [rustup](https://www.rustup.rs/)

```bash
# Get rustup from rustup.rs, then in your `ckb` folder:
rustup override set 1.31.0
rustup component add rustfmt
rustup component add clippy
```

Report new breakage is welcome.

You also need to get the following packages：

* Ubuntu and Debian:

```shell
sudo apt-get install git autoconf flex bison texinfo libtool pkg-config libssl-dev libclang-dev
```

If you are on Ubuntu 18.04, you might run into `'stdarg.h' file not found` error, this is because `librocksdb-sys` fails to find the correct include path. A temporary fix until `librocksdb-sys` fixes this problem is as follows:

```shell
sudo ln -s /usr/lib/gcc/x86_64-linux-gnu/7/include/stdarg.h /usr/include/stdarg.h
sudo ln -s /usr/lib/gcc/x86_64-linux-gnu/7/include/stddef.h /usr/include/stddef.h
```

* Archlinux

```shell
sudo pacman -Sy git autoconf flex bison texinfo libtool pkg-config openssl-1.0 clang
```

If you get openssl related errors in compiling, try the following environment variables to specify openssl-1.0:

```shell
OPENSSL_INCLUDE_DIR=/usr/include/openssl-1.0 OPENSSL_LIB_DIR=/usr/lib/openssl-1.0 cargo build --release
```

* OSX:

```shell
brew install autoconf libtool
```

---

## Build from source & testing

```bash
# get ckb source code
git clone https://github.com/nervosnetwork/ckb.git
cd ckb

# build in release mode
cargo build --release
```

You can run the full test suite, or just run a specific package test:

```bash
# Run the full suite
cargo test --all
# Run a specific package test
cargo test -p ckb-chain
```

---

## Quick Start

### Start Node

Create the default runtime directory:

```shell
cp -r nodes_template/ nodes
```

Use the config file to start the node

```shell
target/release/ckb run
```

It searches config file `ckb.json`, `nodes/default.json` in the shell
working directory in that order. Alternatively, the argument `-c` can specify
the config file used to start the node.

The default config file saves data in `nodes/default/`.

### Use RPC

Find RPC port in the log output, the following command assumes 8114 is used:

```shell
curl -d '{"id": 1, "jsonrpc": "2.0", "method":"get_tip_header","params": []}' \
  -H 'content-type:application/json' 'http://localhost:8114'
```

### Run Miner

Run miner, gets a block template to mine.

```shell
target/release/ckb miner
```

### Advanced

Run multiple nodes in different data directories.

Create the config file for new nodes, for example:

```shell
cp nodes/default.json nodes/node2.json
```

Update `data_dir` configuration in config file to a different directory.

```
"data_dir": "node2"
```

Then start the new node using the new config file

```shell
target/release/ckb -c nodes/node2.json run
```

The option `ckb.chain` configures the chain spec. It accepts a path to the spec JSON file. The directory `nodes_template/spec` has all the pre-defined specs. Please note that nodes with different chain specs may fail to connect with each other.

The chain spec can switch between different PoW engines. Wiki has the [instructions](https://github.com/nervosnetwork/ckb/wiki/PoW-Engines) about how to configure it.
