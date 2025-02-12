#!/usr/bin/env bash

set -euo pipefail

case "$OSTYPE" in
  darwin*)
    if ! type gsed &>/dev/null || ! type ggrep &>/dev/null; then
      echo "GNU sed and grep not found! You can install them via Homebrew" >&2
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

function main() {
  local res=$(find ./ -not -path '*/target/*' -type f -name "*.rs" | xargs grep -H "Relaxed")

  if [ -z "${res}" ]; then
    echo "ok"
    exit 0
  else
    echo "find use Relaxed on code, please check"

    for file in ${res}; do
        printf ${file}
    done

    exit 1
  fi
}

main "$@"
