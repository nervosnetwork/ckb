#!/bin/bash
set -euo pipefail
is_self_runner=$(echo $RUNNER_LABEL | awk -F '-' '{print $1}')
clean_threshold=40000
available_space=$(df -m "$GITHUB_WORKSPACE" | tail -1 | awk '{print $4}')
if [[ $is_self_runner == "self" ]]; then
  export CARGO_TARGET_DIR="$GITHUB_WORKSPACE/../target"
  # export RUSTC_WRAPPER='sccache'
  # export SCCACHE_CACHE_SIZE='20G'
  #clean space when disk full
  if [[ $available_space -lt $clean_threshold ]]; then
    echo "Run clean command"
    cargo clean --target-dir "${CARGO_TARGET_DIR}" || true
  fi
fi
CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-"$GITHUB_WORKSPACE/target"}
case $GITHUB_WORKFLOW in
  ci_linters*)
    echo "ci_linters"
    cargo fmt --version || rustup component add rustfmt
    cargo clippy --version || rustup component add clippy
    make fmt
    make clippy
    git diff --exit-code Cargo.lock
    ;;
  ci_unit_test*)
    echo "ci_unit_tests"
    github_workflow_os=$(echo $GITHUB_WORKFLOW | awk -F '_' '{print $NF}')
    if [[ $github_workflow_os == 'macos' ]]; then
      export CKB_FEATURES="deadlock_detection,with_sentry,portable"
    fi
    make test
    ;;
  ci_benchmarks*)
    echo "ci_benchmarks_test"
    github_workflow_os=$(echo $GITHUB_WORKFLOW | awk -F '_' '{print $NF}')
    if [[ $github_workflow_os == 'macos' ]]; then
      export CKB_BENCH_FEATURES="ci,portable"
    fi
    make bench-test
    ;;
  ci_integration_tests*)
    echo "ci_integration_test"
    github_workflow_os=$(echo $GITHUB_WORKFLOW | awk -F '_' '{print $NF}')
    export BUILD_BUILDID=$GITHUB_RUN_ID
    export ImageOS=$RUNNER_OS
    export BINARY_NAME=${BINARY_NAME:-"ckb"}
    if [[ $github_workflow_os == 'windows' ]]; then
      BINARY_NAME="ckb.exe"
    fi
    if [[ $github_workflow_os == 'macos' ]]; then
      export CKB_FEATURES="deadlock_detection,with_sentry,portable"
    fi
    make CKB_TEST_ARGS="-c 4 --no-report --max-time 7200 " integration
    ;;
  ci_quick_checks*)
    echo "ci_quick_check"
    make check-cargotoml
    make check-whitespaces
    make check-dirty-rpc-doc
    make check-dirty-hashes-toml
    devtools/ci/check-cyclic-dependencies.py
    devtools/ci/check-relaxed.sh
    ;;
  ci_aarch64_build*)
    echo "ci_aarch64_build"
    sudo apt-get install -y gcc-multilib
    sudo apt-get install -y gcc-aarch64-linux-gnu g++-aarch64-linux-gnu clang
    rustup target add aarch64-unknown-linux-gnu
    curl -LO https://www.openssl.org/source/openssl-3.1.3.tar.gz
    tar -xvzf openssl-3.1.3.tar.gz
    cd openssl-3.1.3
    CC=aarch64-linux-gnu-gcc ./Configure linux-aarch64 shared
    CC=aarch64-linux-gnu-gcc make
    cd ..
    export TOP
    export OPENSSL_LIB_DIR=$(pwd)/openssl-3.1.3
    export OPENSSL_INCLUDE_DIR=$(pwd)/openssl-3.1.3/include
    export PKG_CONFIG_ALLOW_CROSS=1
    export CC=gcc
    export CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc
    cargo build --target=aarch64-unknown-linux-gnu --features portable
    ;;
  ci_cargo_deny*)
    echo "ci_security_audit_licenses"
    cargo deny --version || cargo +stable install cargo-deny --locked --version 0.17.0
    make security-audit
    make check-crates
    make check-licenses
    ;;
  *)
    echo -n "unknown"
    ;;
esac
