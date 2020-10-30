#!/usr/bin/env bash

set -euo pipefail

SRC_ROOT=.
ERRCNT=0

case "$OSTYPE" in
    darwin*)
        if ! type gsed &> /dev/null || ! type ggrep &> /dev/null; then
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

        if grep -n -H '{.*path\s*=\s*' $cargo_toml | grep -F -v 'version = "= '"$expected"'"'; then
          printf "Error: Local depedencies in <%s> must specify version \"= %s\"\n" \
              "${cargo_toml}" "${expected}"
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

function search_crate() {
    local crate="$1"
    local source="$2"
    local tmpcnt=0
    local depcnt=0
    local grepopts="-rh"
    tmpcnt=$({\
        ${GREP} ${grepopts} "\(^\| \)extern crate ${crate}\(::\|;\| as \)" "${source}" \
            || true; }\
        | wc -l)
    depcnt=$((depcnt + tmpcnt))
    tmpcnt=$({\
        ${GREP} ${grepopts} "\(^\| \)use ${crate}\(::\|;\| as \)" "${source}" \
            || true; }\
        | wc -l)
    depcnt=$((depcnt + tmpcnt))
    tmpcnt=$({\
        ${GREP} ${grepopts} "\(^\|[ (<]\)\(::\|\)${crate}::" "${source}" \
            || true; }\
        | wc -l)
    depcnt=$((depcnt + tmpcnt))
    printf "${depcnt}"
}

function check_dependencies_for() {
    local deptype="$1"
    for cargo_toml in $(find "${SRC_ROOT}" -type f -name "Cargo.toml"); do
        local pkgroot=$(dirname "${cargo_toml}")
        for dependency_original in $(${SED} -n "/^\[${deptype}\]/,/^\[/p" "${cargo_toml}" \
                | { ${GREP} -v "^\(\[\|[ ]*$\|[ ]*#\)" || true; } \
                | ${SED} -n "s/\([^ =]*\).*/\1/p"); do
            local dependency=$(printf "${dependency_original}" | tr '-' '_')
            local tmpcnt=0
            local depcnt=0
            local srcdir=
            local buildrs=
            case "${deptype}" in
                "dependencies" | "dev-dependencies")
                    srcdir="${pkgroot}/src"
                    if [ ! -d "${srcdir}" ]; then
                        srcdir="${pkgroot}"
                    fi
                    tmpcnt=$(search_crate "${dependency}" "${srcdir}")
                    depcnt=$((depcnt + tmpcnt))
                    ;;
                "build-dependencies")
                    buildrs="${pkgroot}/build.rs"
                    tmpcnt=$(search_crate "${dependency}" "${buildrs}")
                    depcnt=$((depcnt + tmpcnt))
                    ;;
                *)
                    :
                    ;;
            esac
            if [ "${deptype}" = "dev-dependencies" ]; then
                for subdir in "tests" "benches" "examples"; do
                    srcdir="${pkgroot}/${subdir}"
                    if [ -d "${srcdir}" ]; then
                        tmpcnt=$(search_crate "${dependency}" "${srcdir}")
                        depcnt=$((depcnt + tmpcnt))
                    fi
                done
            fi
            if [ "${depcnt}" -eq 0 ]; then
                case "${dependency}" in
                    phf)
                        # We cann't handle these crates.
                        printf "Warn: [%s::%s] in <%s>\n" \
                            "${deptype}" "${dependency}" "${pkgroot}"
                        ;;
                    *)
                        printf "Error: [%s::%s] in <%s>\n" \
                            "${deptype}" "${dependency}" "${pkgroot}"
                        ERRCNT=$((ERRCNT + 1))
                        ;;
                esac
            fi
            if [ "${deptype}" = "dev-dependencies" ]; then
                tmpcnt=$(${GREP} -c "^${dependency_original}[^a-zA-Z0-9=_-]*=" "${cargo_toml}")
                if [ "${tmpcnt}" -gt 1 ]; then
                    printf "Warn: [%s::%s] in <%s>, twice\n" \
                        "${deptype}" "${dependency}" "${pkgroot}"
                fi
            fi
        done
    done
}

function check_dependencies() {
    check_dependencies_for "dependencies"
    check_dependencies_for "build-dependencies"
    check_dependencies_for "dev-dependencies"
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
