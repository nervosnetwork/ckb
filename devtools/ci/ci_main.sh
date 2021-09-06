#!/bin/bash
set -u
set +e
is_self_runner=`echo $RUNNER_LABEL | awk -F '-' '{print $1}'`
if [[ $is_self_runner == "self" ]];then
  CARGO_TARGET_DIR=$GITHUB_WORKSPACE/../target
fi
CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-"$GITHUB_WORKSPACE/target"}
case $GITHUB_WORKFLOW in
  ci_linters*)
    echo "Hellow ci_linters!"
    cargo fmt --version ||  rustup component add rustfmt
    argo clippy --version ||  rustup component add clippy
    make fmt
    make clippy
    git diff --exit-code Cargo.lock
    EXIT_CODE="${PIPESTATUS[0]}"
    ;;
  ci_unit_test*)
    echo "ci_unit_tests"
    make test
    EXIT_CODE="${PIPESTATUS[0]}"
    ;;
  ci_benchmarks*)
    echo "ci_benchmarks_test"
    make bench-test
    EXIT_CODE="${PIPESTATUS[0]}"
    ;;
ci_integration_tests*)
    echo "ci_integration_test"
    github_workflow_os=`echo $GITHUB_WORKFLOW | awk -F '_' '{print $NF}'`
    git submodule update --init
    cp -f Cargo.lock test/Cargo.lock
    cargo build --features deadlock_detection --target-dir $CARGO_TARGET_DIR
    rm -rf test/target && ln -snf ${CARGO_TARGET_DIR} test/target
    cd test
    if [[ $github_workflow_os == 'windows' ]];then
      cargo run -- --bin ${CARGO_TARGET_DIR}/debug/ckb.exe --log-file target/integration.log ${CKB_TEST_ARGS}
    else
      cargo run -- --bin ${CARGO_TARGET_DIR}/debug/ckb --log-file target/integration.log ${CKB_TEST_ARGS}
    fi
    EXIT_CODE="${PIPESTATUS[0]}"
    ;;
  ci_quick_checks*)
    echo "ci_quick_check"
    make check-cargotoml
    make check-whitespaces
    make check-dirty-rpc-doc
    make check-dirty-hashes-toml
    devtools/ci/check-cyclic-dependencies.py
    EXIT_CODE="${PIPESTATUS[0]}"
    ;;
  ci_wasm_build*)
    echo "ci_WASM_build"
    rustup target add wasm32-unknown-unknown
    make wasm-build-test
    EXIT_CODE="${PIPESTATUS[0]}"
    ;;
  ci_cargo_deny*)
    echo "ci_security_audit_licenses"
    cargo deny --version || cargo install cargo-deny --locked
    make security-audit
    make check-crates
    make check-licenses
    EXIT_CODE="${PIPESTATUS[0]}"
    ;;
  *)
    echo -n "unknown"
    ;;
esac
exit $EXIT_CODE