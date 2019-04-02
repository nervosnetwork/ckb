# Build CKB

## Build dependencies

CKB is currently tested mainly with `stable-1.33.0` on Linux and macOS.

We recommend installing Rust through [rustup](https://www.rustup.rs/)

```bash
# Get rustup from rustup.rs, then in your `ckb` folder:
rustup override set 1.33.0
rustup component add rustfmt
rustup component add clippy
```

Report new breakage is welcome.

You also need to get the following packages：

#### Ubuntu and Debian

```shell
sudo apt-get install git gcc libc6-dev pkg-config libssl-dev libclang-dev clang
```

#### Arch Linux

```shell
sudo pacman -Sy git gcc pkgconf clang
```

#### macOS

```shell
brew install autoconf libtool
```

---

## Build from source

```bash
# get ckb source code
git clone https://github.com/nervosnetwork/ckb.git
cd ckb

# build in release mode
make build
```

This will build the executable `target/release/ckb`.
