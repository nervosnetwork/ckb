# Get CKB

## Download from Releases

We will publish binaries for each release via [Github Releases](https://github.com/nervosnetwork/ckb/releases). If your system
is listed there, you can download the package directory.

There is also a repository [ckb-nightly](https://github.com/nervosnetwork/ckb-nightly/releases) containing the nightly builds from the develop
branch.


## Build from Source

### Install Build Dependencies

CKB is currently tested mainly with `stable-1.34.0` on Linux and macOS.

We recommend installing Rust through [rustup](https://www.rustup.rs/)

```bash
# Get rustup from rustup.rs, then in your `ckb` folder:
rustup override set 1.34.0
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

### Build from source

The `master` branch is regularly built and tested. It is always the latest released version. The default checked out branch `develop` is the latest version in development.

It is recommended to build a version from master.

```bash
# get ckb source code
git clone https://github.com/nervosnetwork/ckb.git
cd ckb
git checkout master

# build in release mode
make build
```

This will build the executable `target/release/ckb`. Please add the directory
to `PATH` or copy/link the file into a directory already in the `PATH`.

```bash
export PATH="$(pwd)/target/release:$PATH"
# or
# ln -snf "$(pwd)/target/release/ckb" /usr/local/bin/ckb
```

It is easy to switch to a history version and build, for example, check out
v0.8.0:

```bash
git checkout -b branch-v0.8.0 v0.8.0
```
