#!/bin/bash
set -ev

cargo sweep --version || cargo install --git https://github.com/holmgr/cargo-sweep --rev 4770deda37a2203c783e301b8c0c895964e8971e

if [ "$FMT" = true ]; then
  cargo fmt --version || rustup component add rustfmt
fi

if [ "$CHECK" = true ]; then
  cargo clippy --version || rustup component add clippy
fi
