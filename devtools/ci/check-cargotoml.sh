#!/usr/bin/env bash

set -euo pipefail

SRC_ROOT=.
ERRCNT=0

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

CARGOS=$(find "${SRC_ROOT}" -type f -name "Cargo.toml" -not -path '*/target/*')

function check_package_name() {
  local regex_to_cut_pkgname='s/^\[\(package\)\]\nname\(\|[ ]\+\)=\(\|[ ]\+\)"\(.\+\)"/\4/p'
  for cargo_toml in ${CARGOS}; do
    local pkgname=$(${SED} -n -e N -e "${regex_to_cut_pkgname}" "${cargo_toml}")
    if [ -z "${pkgname}" ]; then
      printf "Error: No package name in <%s>\n" "${cargo_toml}"
      ERRCNT=$((ERRCNT + 1))
    elif [[ "${pkgname}" =~ ^ckb- ]] || [ "${pkgname}" = "ckb" ]; then
      :
    else
      printf "Error: Package name in <%s> is not with prefix 'ckb-' (actual: '%s')\n" \
        "${cargo_toml}" "${pkgname}"
      ERRCNT=$((ERRCNT + 1))
    fi
  done
}

function check_version() {
  local regex_to_cut_version='s/^version = "\(.*\)"$/\1/p'
  local expected=$(${SED} -n "${regex_to_cut_version}" "${SRC_ROOT}/Cargo.toml")
  for cargo_toml in ${CARGOS}; do
    local tmp=$(${SED} -n "${regex_to_cut_version}" "${cargo_toml}")
    if [ "${expected}" != "${tmp}" ]; then
      printf "Error: Version in <%s> is not right (expect: '%s', actual: '%s')\n" \
        "${cargo_toml}" "${expected}" "${tmp}"
      ERRCNT=$((ERRCNT + 1))
    fi

    if grep -n -H '{.*path\s*=\s*' $cargo_toml | grep -F -v 'version = "= '"$expected"'"'; then
      printf "Error: Local dependencies in <%s> must specify version \"= %s\"\n" \
        "${cargo_toml}" "${expected}"
      ERRCNT=$((ERRCNT + 1))
    fi
  done
}

function check_license() {
  local regex_to_cut_license='s/^license = "\(.*\)"$/\1/p'
  local expected=$(${SED} -n "${regex_to_cut_license}" "${SRC_ROOT}/Cargo.toml")
  for cargo_toml in ${CARGOS}; do
    local tmp=$(${SED} -n "${regex_to_cut_license}" "${cargo_toml}")
    if [ "${expected}" != "${tmp}" ]; then
      printf "Error: License in <%s> is not right (expect: '%s', actual: '%s')\n" \
        "${cargo_toml}" "${expected}" "${tmp}"
      ERRCNT=$((ERRCNT + 1))
    fi
  done
}

function check_cargo_publish() {
  for cargo_toml in ${CARGOS}; do
    if ! grep -q '^description =' "${cargo_toml}"; then
      echo "Error: Require description in <${cargo_toml}>"
      ERRCNT=$((ERRCNT + 1))
    fi
    if ! grep -q '^homepage =' "${cargo_toml}"; then
      echo "Error: Require homepage in <${cargo_toml}>"
      ERRCNT=$((ERRCNT + 1))
    fi
    if ! grep -q '^repository =' "${cargo_toml}"; then
      echo "Error: Require repository in <${cargo_toml}>"
      ERRCNT=$((ERRCNT + 1))
    fi
  done
}

function check_dependencies() {
  if ! type cargo-shear &> /dev/null
  then
      # lock version to avoid breaking building now
      cargo install cargo-shear --version 1.1.10
  fi
  cargo shear
}

function main() {
  echo "[BEGIN] Checking Cargo.toml ..."
  check_package_name
  check_version
  check_license
  check_cargo_publish
  check_dependencies
  echo "[ END ] Found ${ERRCNT} errors."
  if [ "${ERRCNT}" -ne 0 ]; then
    exit 1
  fi
}

main "$@"
