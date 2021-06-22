#!/bin/bash

set -eu

git merge --no-ff --no-commit -s ours master
git checkout master -- CHANGELOG.md .github/workflows/package.yaml
