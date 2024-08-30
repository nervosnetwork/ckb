#!/usr/bin/env bash

set -euo pipefail

case "$OSTYPE" in
  darwin*)
    if ! type gsed &>/dev/null || ! type ggrep &>/dev/null; then
      echo "GNU sed and grep not found! You can install via Homebrew" >&2
      echo >&2
      echo "    brew install grep gnu-sed" >&2
      exit 1
    fi

    SED=gsed
    GREP=ggrep
    ;;
  *)
    SED=sed
    GREP=grep
    ;;
esac

find ./ -not -path '*/target/*' -type f -name "*.rs" | xargs grep -H "Relaxed"

if [ $? -eq 0 ]; then
    echo "find use Relaxed on code, please check"
    exit 1
fi
