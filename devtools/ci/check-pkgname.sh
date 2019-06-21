#!/usr/bin/env bash

set -euo pipefail

case "$OSTYPE" in
    darwin*)
        sed=gsed
        ;;
    *)
        sed=sed
        ;;
esac

function main() {
    local regex_to_cut_pkgname='s/^\[\(package\)\]\nname\(\|[ ]\+\)=\(\|[ ]\+\)"\(.\+\)"/\4/p'
    local errcnt=0

    for cargo_toml in $(find . -type f -name "Cargo.toml"); do
        local pkgname=$(${sed} -n -e N -e "${regex_to_cut_pkgname}" "${cargo_toml}")
        if [ -z "${pkgname}" ]; then
            echo "No package name for <${cargo_toml}>."
            errcnt=$((errcnt + 1))
        elif [[ "${pkgname}" =~ ^ckb- ]] || [ "${pkgname}" = "ckb" ]; then
            :
        else
            echo "Package name [${pkgname}] for <${cargo_toml}> is not with prefix 'ckb-'."
            errcnt=$((errcnt + 1))
        fi
    done

    exit ${errcnt}
}

main "$@"
