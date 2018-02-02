# [Nervos]() - Common Knowledge Base

[![CircleCI](https://circleci.com/gh/NervosFoundation/nervos.svg?style=svg&circle-token=5e9e1e761685962d44dffa25af631e9c56151cea)](https://circleci.com/gh/NervosFoundation/nervos)

----

## About Nervos

Nervos is a Common Knowledge Base blockchain system, it defines a suite of scalable and interoperable blockchain protocols for human to create the [common knowledge](https://en.wikipedia.org/wiki/Common_knowledge_(logic)), a self-evolving synchronized community with novel economic model, stable coin, identity and more.

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
