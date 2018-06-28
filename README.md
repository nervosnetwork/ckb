<img src="https://raw.githubusercontent.com/poshboytl/tuchuang/master/nervos-logo-dark.png" width="256">

# [Nervos CKB](http://nervos.org) - The Common Knowledge Base

[![TravisCI](https://travis-ci.com/NervosFoundation/ckb.svg?token=y9uR6ygmT3geQaMJ4jpJ&branch=develop)](https://travis-ci.com/NervosFoundation/ckb)

---

## About Nervos CKB

Nervos CKB is the first Common Knowledge Base to facilitate the creation and storage of [common knowledge](<https://en.wikipedia.org/wiki/Common_knowledge_(logic)>) of our society.

Nervos project defines a suite of scalable and interoperable blockchain protocols. Nervos CKB uses those protocols to create a self-evolving distributed network with novel economic model, data model and more.

---

## Build dependencies

**Rust Nightly is required**. Nervos is currently tested mainly with `nightly-2018-05-23`.

We recommend installing Rust through [rustup](https://www.rustup.rs/)

```bash
# Get rustup from rustup.rs, then in your `nervos` folder:
rustup override set nightly-2018-05-23
rustup component add rustfmt-preview --toolchain=nightly-2018-05-23
```

we would like to track `nightly`, report new breakage is welcome.

you alse need to get the following packages：

* Ubuntu and Debian:

```shell
sudo apt-get install git autoconf flex bison texinfo libtool
```

* OSX:

```shell
brew install autoconf libtool
```

---

## Build from source

```bash
# download Nervos
$ git clone https://github.com/NervosFoundation/nervos.git
$ cd nervos

# build in release mode
$ cargo build --release
```

---

## Quick Start

### Start Node

```shell
target/release/nervos
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
cargo install protobuf --force --vers 1.4.3
```

Generate code from proto definition:

```shell
make proto
```

### Development running

Run multiple nodes:

```shell
$ cargo run -- --data-dir=/tmp/node1
$ cargo run -- --data-dir=/tmp/node2
```

Modify development config file
```shell
cp src/config/development.toml /tmp/node1
```
