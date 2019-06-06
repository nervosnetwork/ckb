# Get CKB

## Download from Releases

We will publish binaries for each release via [Github Releases]. If your system
is listed there, you can download the package directory.

CKB releases are signed. It is wise and more secure to check out for their [integrity](integrity-check.md).

[Github Releases]: https://github.com/nervosnetwork/ckb/releases

CentOS users please use the `x86_64-unknown-centos-gnu` package, which also
requires OpenSSL 1.0 to run:

```shell
sudo yum install openssl-libs
```

The Windows packages are for experiments only, they have significant
performance issues, we don't recommend to use them in production environment.
They requires *The Visual C++ Redistributable Packages*, which can be downloaded
under section *Other Tools and Frameworks*
[here](https://visualstudio.microsoft.com/downloads/) or
[here](https://www.microsoft.com/en-us/download/details.aspx?id=48145).

We also provides docker images, see [how to run CKB with docker](run-ckb-with-docker.md).

## Build from Source

### Install Build Dependencies

CKB requires Rust to build. We recommend installing [rustup](https://www.rustup.rs/) to manage Rust versions.

The required Rust version is saved in the file `rust-toolchain`. If rustup is
available, it will pick the right version automatically.

You also need to get the following packages：

#### Ubuntu and Debian

```shell
sudo apt-get install -y git gcc libc6-dev pkg-config libssl-dev libclang-dev clang
```

#### Arch Linux

```shell
sudo pacman -Sy git gcc pkgconf clang
```

#### macOS

```shell
brew install autoconf libtool
```

#### CentOS

```shell
sudo yum install -y centos-release-scl
sudo yum install -y git make gcc-c++ openssl-devel llvm-toolset-7
```

Start a shell enabling clang

```shell
scl enable llvm-toolset-7 bash
```

Remember to run following commands in this console.

#### Windows

Install Visual Studio with Desktop C++ workload, and install following
packages via [Chocolatey](https://chocolatey.org)

```
choco install -y llvm msys2
```

### Add Environment Variables

If your OS contains pre-compiled `rocksdb` or `snappy` libraries,
you may setup `ROCKSDB_LIB_DIR` and/or `SNAPPY_LIB_DIR` environment variable
to point to a directory with these libraries.
This will significantly reduce compile time.

```shell
export ROCKSDB_LIB_DIR=/usr/local/lib
export SNAPPY_LIB_DIR=/usr/local/lib
```

### Build from source

The `master` branch is regularly built and tested. It is always the latest
released version. The default checked out branch `develop` is the latest
version in active development.

It is recommended to build a version from master.

You can download the source code of [master
branch](https://github.com/nervosnetwork/ckb/archive/master.zip) from GitHub,
or a history version from [GitHub Releases].

You also can choose to clone the code via git:

```bash
# get ckb source code
git clone https://github.com/nervosnetwork/ckb.git
cd ckb
git checkout master
```

It is easy to switch to a history version and build, for example, check out
v0.12.2.

```bash
git checkout -b branch-v0.12.2 v0.12.2
```

Run `make prod` inside the source code directory. It will build the executable
`target/release/ckb`. Please add the directory to `PATH` or copy/link the file
into a directory already in the `PATH`.

```bash
export PATH="$(pwd)/target/release:$PATH"
# or
# ln -snf "$(pwd)/target/release/ckb" /usr/local/bin/ckb
```

In Windows, use `cargo build --release` instead and the executable is
`target/release/ckb.exe`.
