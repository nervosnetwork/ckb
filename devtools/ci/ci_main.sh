#!/bin/bash
set -u
set +e
func_ci() {
 name=${1}
 if [ $name == "ci_unit_tests" ];then
    echo "ci_unit_tests"
    make test
 fi
 if [ $name == "ci_benchmarks_test" ];then
    echo "ci_benchmarks_test"
    make bench-test
 fi
 if [ $name == "ci_integration_test" ];then
    echo "ci_integration_test"
    cd test
    cargo run -- --log-file target/integration.log $CKB_TEST_ARGS
 fi
 if [ $name == "ci_quick_check" ];then
    echo "ci_quick_check"
    make check-cargotoml
    make check-whitespaces
    make check-dirty-rpc-doc
    make check-dirty-hashes-toml
    devtools/ci/check-cyclic-dependencies.py
 fi
 if [ $name == "ci_linters" ];then
    echo "ci_linters"
    cargo fmt --version ||  rustup component add rustfmt
    cargo clippy --version ||  rustup component add clippy
    make fmt
    make clippy
    git diff --exit-code Cargo.lock
 fi
 if [ $name == "ci_security_audit_licenses" ];then
    echo "ci_security_audit_licenses"
    cargo deny --version || cargo install cargo-deny --locked
    make security-audit
    make check-crates
    make check-licenses
 fi
 if [ $name == "ci_WASM_build" ];then
    echo "ci_WASM_build"
    rustup target add wasm32-unknown-unknown
    make wasm-build-test
 fi
 EXIT_CODE=$?
 echo $EXIT_CODE
}


name="$1"
if [ $EVENT_NAME == "push" ];then
   COMMIT_SHA=$COMMIT_SHA
   MESSAGE="$COMMIT_MESSAGE"
fi
if [ $EVENT_NAME == "pull_request" ];then
    COMMIT_SHA=$PR_COMMIT_SHA
    MESSAGE="$PR_COMMONS_BODY"
fi
nervosnetwork_actor_list='"janx", "doitian", "quake", "xxuejie", "zhangsoledad", "jjyr", "TheWaWaR", "driftluo", "keroro520", "yangby-cryptape","liya2017"'
echo $MESSAGE | grep -q "skip ci"
if [[ $? -eq 0 ]]; then
   if [[ $EVENT_NAME == "push" ]]  || [[ $EVENT_NAME == "pull_request"  &&  $nervosnetwork_actor_list =~ $ACTOR ]];then
   echo "skip ci"
   EXIT_CODE="0"
   fi
else
  func_ci $name
  echo "EXIT_CODE is "$EXIT_CODE
fi
EXIT_STATUS=$EXIT_CODE
set -e
if [ "$EXIT_STATUS" = 0 ]; then
    echo "Check whether the ci succeeds"
else
    echo "Fail the ci"
fi
exit $EXIT_STATUS