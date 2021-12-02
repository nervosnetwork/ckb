#!/bin/bash
set -euo pipefail
is_self_runner=`echo $RUNNER_LABEL | awk -F '-' '{print $1}'`
clean_threshold=40000
available_space=`df -m "$GITHUB_WORKSPACE" | tail -1 | awk '{print $4}'`
if [[ $is_self_runner == "self" ]];then
  CARGO_TARGET_DIR=$GITHUB_WORKSPACE/../target
  #clean space when disk full
  if [[ $available_space -lt $clean_threshold ]]; then
          echo "Run clean command"
          cargo clean --target-dir "${CARGO_TARGET_DIR}" || true
  fi
fi
CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-"$GITHUB_WORKSPACE/target"}
EXIT_CODE=0
case $GITHUB_WORKFLOW in
  ci_linters*)
    echo "ci_linters"
    cargo fmt --version ||  rustup component add rustfmt
    cargo clippy --version ||  rustup component add clippy
    make fmt
    make clippy
    git diff --exit-code Cargo.lock
    ;;
  ci_unit_test*)
    echo "ci_unit_tests"
    make test
    ;;
  ci_benchmarks*)
    echo "ci_benchmarks_test"
    make bench-test
    ;;
ci_integration_tests*)
    echo "ci_integration_test"
    export BUILD_BUILDID=$GITHUB_RUN_ID
    export ImageOS=$RUNNER_OS
    make CKB_TEST_SEC_COEFFICIENT=5 CKB_TEST_ARGS="-c 4 --no-report" integration
    ;;
  ci_quick_checks*)
    echo "ci_quick_check"
    make check-cargotoml
    make check-whitespaces
    make check-dirty-rpc-doc
    make check-dirty-hashes-toml
    devtools/ci/check-cyclic-dependencies.py
    ;;
  ci_wasm_build*)
    echo "ci_WASM_build"
    rustup target add wasm32-unknown-unknown
    make wasm-build-test
    ;;
  ci_aarch64_build*)
    echo "ci_aarch64_build"
    sudo apt-get install -y gcc-multilib
    sudo apt-get install -y gcc-aarch64-linux-gnu g++-aarch64-linux-gnu clang
    rustup target add aarch64-unknown-linux-gnu
    curl -LO https://www.openssl.org/source/openssl-1.1.1.tar.gz
    tar -xvzf openssl-1.1.1.tar.gz
    cd openssl-1.1.1
    CC=aarch64-linux-gnu-gcc ./Configure linux-aarch64 shared
    CC=aarch64-linux-gnu-gcc make
    cd ..
    export TOP
    export OPENSSL_LIB_DIR=$(pwd)/openssl-1.1.1
    export OPENSSL_INCLUDE_DIR=$(pwd)/openssl-1.1.1/include
    export PKG_CONFIG_ALLOW_CROSS=1
    export CC=gcc
    export CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc
    cargo build --target=aarch64-unknown-linux-gnu
    ;;
  ci_cargo_deny*)
    echo "ci_security_audit_licenses"
    cargo deny --version || cargo +stable install cargo-deny --locked
    make security-audit
    make check-crates
    make check-licenses
    ;;
  *)
    echo -n "unknown"
    ;;
esac
