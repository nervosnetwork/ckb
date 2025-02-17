#!/bin/bash

set -eu

git merge --no-ff --no-commit -s ours master
git checkout master -- \
    CHANGELOG.md src/main.rs resource/specs/testnet.toml resource/specs/mainnet.toml \
    resource/specs/mainnet.toml.asc .github/workflows/package.yaml \
    util/constant/src/default_assume_valid_target.rs \
    util/constant/src/latest_assume_valid_target.rs
