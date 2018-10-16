<img src="https://raw.githubusercontent.com/poshboytl/tuchuang/master/nervos-logo-dark.png" width="256">

# [Nervos CKB](http://nervos.org) - The Common Knowledge Base

[![TravisCI](https://travis-ci.com/NervosFoundation/ckb.svg?token=y9uR6ygmT3geQaMJ4jpJ&branch=develop)](https://travis-ci.com/NervosFoundation/ckb)

---

## About Nervos CKB

Nervos CKB is the layer 1 of Nervos Network, a public blockchain with PoW and cell model.

Nervos project defines a suite of scalable and interoperable blockchain protocols. Nervos CKB uses those protocols to create a self-evolving distributed network with novel economic model, data model and more.

---

## Build dependencies

Nervos is currently tested mainly with `stabe-1.29.2`.

We recommend installing Rust through [rustup](https://www.rustup.rs/)

```bash
# Get rustup from rustup.rs, then in your `ckb` folder:
rustup override set 1.29.2
rustup component add rustfmt-preview
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


* OSX:

```shell
brew install autoconf libtool
```

---

## Build from source & testing

```bash
# download Nervos
$ git clone https://github.com/NervosFoundation/ckb.git
$ cd ckb

# build in release mode
$ cargo build --release
```

You can run the full test suite, or just run a specific package test:
```bash
# Run the full suite
make test
# Run a specific package test
RUSTFLAGS="--cfg ckb_test" cargo test --package ckb-chain
```

---

## Quick Start

### Start Node

```shell
target/release/ckb
```

### Send Transaction via RPC

Find RPC port in the log output, the following command assumes 3030 is used:

```shell
curl -d '{"id": 2, "jsonrpc": "2.0", "method":"send_transaction","params": [{"version":2, "inputs":[], "outputs":[], "groupings":[]}]}' \
  -H 'content-type:application/json' 'http://localhost:3030'
```

### Protobuf Code Generation

Install protobuf:

```shell
cargo install protobuf-codegen --force --vers 2.0.4
```

Generate code from proto definition:

```shell
make proto
```

### Development running

Run multiple nodes:

```shell
$ cargo run -- run --data-dir=/tmp/node1
$ cargo run -- run --data-dir=/tmp/node2
```

Modify development config file
```shell
cp src/config/development.toml /tmp/node1/config.toml
```
