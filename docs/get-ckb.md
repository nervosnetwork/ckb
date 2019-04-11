# Get CKB

## Download from Releases

We will publish binaries for each release via [Github Releases](https://github.com/nervosnetwork/ckb/releases). If your system
is listed there, you can download the package directory.


## Build from Source

### Install Build Dependencies

CKB is currently tested mainly with `stable-1.33.0` on Linux and macOS.

We recommend installing Rust through [rustup](https://www.rustup.rs/)

```bash
# Get rustup from rustup.rs, then in your `ckb` folder:
rustup override set 1.33.0
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

### Build from master branch

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

```base
export PATH="$(pwd)/target/release:$PATH"
# or
# ln -snf "$(pwd)/target/release/ckb" /usr/local/bin/ckb
```
