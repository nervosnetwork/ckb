#!/bin/bash

if [ "$1" = run ]; then
  docker run --rm -it -v $(pwd):/ckb centos:7 bash /ckb/devtools/ci/centos-docker.sh
  devtools/ci/package.sh target/release/ckb
  exit 0
fi

cd /ckb

set -eu

yum install -y centos-release-scl
yum install -y git curl make gcc-c++ openssl-devel llvm-toolset-7
curl https://sh.rustup.rs -sSf | sh -s -- --default-toolchain 1.34.2 -y

source $HOME/.cargo/env
scl enable llvm-toolset-7 'make prod'
