#!/bin/bash

set -eu

git merge --no-ff --no-commit -s ours master
git checkout master -- CHANGELOG.md db/src/db.rs resource/specs/testnet.toml .travis.yml azure-pipelines.yml
