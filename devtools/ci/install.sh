#!/bin/bash
set -ev

cargo sweep --version || cargo install --git https://github.com/holmgr/cargo-sweep --rev 4770deda37a2203c783e301b8c0c895964e8971e

SUB_JOB_NUMBER="${TRAVIS_JOB_NUMBER##*.}"
# Run fmt and check evenly between osx and linux
if (( TRAVIS_BUILD_NUMBER % 2 == SUB_JOB_NUMBER - 1 )); then
  cargo fmt --version || rustup component add rustfmt
  cargo clippy --version || rustup component add clippy
  cargo audit --version || cargo install cargo-audit
fi
