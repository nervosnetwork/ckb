#!/bin/bash

set -eu

git merge --no-ff --no-commit -s ours master
git checkout master -- CHANGELOG.md src/main.rs resource/specs/testnet.toml resource/specs/mainnet.toml resource/specs/mainnet.toml.asc .github/workflows/package.yaml
