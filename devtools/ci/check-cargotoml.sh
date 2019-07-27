#!/usr/bin/env bash

set -euo pipefail

SRC_ROOT=.
ERRCNT=0

case "$OSTYPE" in
    darwin*)
        SED=gsed
        ;;
    *)
        SED=sed
        ;;
esac

function check_package_name() {
    local regex_to_cut_pkgname='s/^\[\(package\)\]\nname\(\|[ ]\+\)=\(\|[ ]\+\)"\(.\+\)"/\4/p'
    for cargo_toml in $(find "${SRC_ROOT}" -type f -name "Cargo.toml"); do
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
    for cargo_toml in $(find "${SRC_ROOT}" -type f -name "Cargo.toml"); do
        local tmp=$(${SED} -n "${regex_to_cut_version}" "${cargo_toml}")
        if [ "${expected}" != "${tmp}" ]; then
            printf "Error: Version in <%s> is not right (expect: '%s', actual: '%s')\n" \
                "${cargo_toml}" "${expected}" "${tmp}"
            ERRCNT=$((ERRCNT + 1))
        fi
    done
}

function check_license() {
    local regex_to_cut_license='s/^license = "\(.*\)"$/\1/p'
    local expected=$(${SED} -n "${regex_to_cut_license}" "${SRC_ROOT}/Cargo.toml")
    for cargo_toml in $(find "${SRC_ROOT}" -type f -name "Cargo.toml"); do
        local tmp=$(${SED} -n "${regex_to_cut_license}" "${cargo_toml}")
        if [ "${expected}" != "${tmp}" ]; then
            printf "Error: License in <%s> is not right (expect: '%s', actual: '%s')\n" \
                "${cargo_toml}" "${expected}" "${tmp}"
            ERRCNT=$((ERRCNT + 1))
        fi
    done
}

function check_dependencies() {
    for cargo_toml in $(find "${SRC_ROOT}" -type f -name "Cargo.toml"); do
        local pkgroot=$(dirname "${cargo_toml}")
        for dependency in $(sed -n '/^\[dependencies\]/,/^\[/p' "${cargo_toml}" \
                | { grep -v "^\(\[\|[ ]*$\|[ ]*#\)" || true; } \
                | sed -n "s/\([^ =]*\).*/\1/p" \
                | tr '-' '_'); do
            local depcnt=0
            local srcdir="${pkgroot}/src"
            if [ ! -d "${srcdir}" ]; then
                srcdir="${pkgroot}"
            fi
            tmpcnt=$({\
                grep -rh "\(^\| \)use ${dependency}\(::\|;\)" "${srcdir}" \
                    || true; }\
                | wc -l)
            depcnt=$((depcnt + tmpcnt))
            tmpcnt=$({\
                grep -rh "[ (<]\(::\|\)${dependency}::" "${srcdir}" \
                    || true; }\
                | wc -l)
            depcnt=$((depcnt + tmpcnt))
            if [ "${depcnt}" -eq 0 ]; then
                case "${dependency}" in
                    serde)
                        tmpcnt=$({\
                            grep -rh "serde_derive" "${cargo_toml}" \
                                || true; }\
                            | wc -l)
                        if [ "${tmpcnt}" -eq 0 ]; then
                            printf "Error: [%s] in <%s>\n" "${dependency}" "${pkgroot}"
                            ERRCNT=$((ERRCNT + 1))
                        fi
                        ;;
                    generic_channel | phf)
                        # We cann't handle these crates.
                        printf "Warn: [%s] in <%s>\n" "${dependency}" "${pkgroot}"
                        ;;
                    *)
                        printf "Error: [%s] in <%s>\n" "${dependency}" "${pkgroot}"
                        ERRCNT=$((ERRCNT + 1))
                        ;;
                esac
            fi
        done
    done
}

function main() {
    echo "[BEGIN] Checking Cargo.toml ..."
    check_package_name
    check_version
    check_license
    check_dependencies
    echo "[ END ] Found ${ERRCNT} errors."
    if [ "${ERRCNT}" -ne 0 ]; then
        exit 1
    fi
}

main "$@"
