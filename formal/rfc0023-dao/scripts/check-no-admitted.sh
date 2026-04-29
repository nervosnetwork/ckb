#!/usr/bin/env bash
set -euo pipefail

if grep -R "Admitted" theories | grep -v "no Admitted"; then
  echo "Admitted is not allowed"
  exit 1
fi

echo "No Admitted found - OK"
