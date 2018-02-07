# [Nervos CKB]() - The Common Knowledge Base

[![CircleCI](https://circleci.com/gh/NervosFoundation/nervos.svg?style=svg&circle-token=5e9e1e761685962d44dffa25af631e9c56151cea)](https://circleci.com/gh/NervosFoundation/nervos)

----

## About Nervos CKB

Nervos CKB is the first Common Knowledge Base to facilitate the creation and storage of [common knowledge](https://en.wikipedia.org/wiki/Common_knowledge_(logic)) of our society.

Nervos project defines a suite of scalable and interoperable blockchain protocols. Nervos CKB uses those protocols to create a self-evolving distributed network with novel economic model, data model and more.

----

## Build dependencies

**Rust Nightly is required**. Nervos is currently tested mainly with `nightly-2018-01-23`.

We recommend installing Rust through [rustup](https://www.rustup.rs/)

```bash
# Get rustup from rustup.rs, then in your `Nervos` folder:
rustup override set nightly-2018-01-23
rustup component add rustfmt-preview --toolchain=nightly-2018-01-23
```

we would like to track `nightly`, report new breakage is welcome.

----

## Build from source

You should install The PBC library first: https://crypto.stanford.edu/pbc/ .

```bash
# download Nervos
$ git clone https://github.com/NervosFoundation/nervos.git
$ cd nervos

# build in release mode
$ cargo build --release
```
