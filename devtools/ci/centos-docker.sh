#!/bin/bash

if [ "$1" = run ]; then
  docker run --rm -it -v $(pwd):/ckb nervos/ckb-docker-builder:centos-7-rust-1.34.2 bash /ckb/devtools/ci/centos-docker.sh
  devtools/ci/package.sh target/release/ckb
  exit 0
fi

cd /ckb

set -eu

scl enable llvm-toolset-7 'make prod'
