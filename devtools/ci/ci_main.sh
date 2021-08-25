#!/bin/bash
set -u
set +e
CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-"$workspace/target"}
#function for different test by name
func_ci() {
 name=${1}
 if [ $name == "ci_unit_tests" ];then
    echo "ci_unit_tests"
    make test
    EXIT_CODE=$?
 fi

 if [ $name == "ci_benchmarks_test" ];then
    echo "ci_benchmarks_test"
    make bench-test
    EXIT_CODE=$?
 fi

 if [ $name == "ci_integration_test" ];then
    echo "ci_integration_test"
    git submodule update --init
    cp -f Cargo.lock test/Cargo.lock
    cargo build --release --target-dir $CARGO_TARGET_DIR
    rm -rf test/target && ln -snf ${CARGO_TARGET_DIR} test/target
    test_id=$(date +"%Y%m%d-%H%M%S")
    echo $test_id
    test_tmp_dir=${CKB_INTEGRATION_TEST_TMP:-"$workspace/target/ckb-test/${test_id}"}
    mkdir -p "${test_tmp_dir}"
    export CKB_INTEGRATION_TEST_TMP="${test_tmp_dir}"
    test_log_file="${test_tmp_dir}/integration.log"
    cd test
    cargo run -- --bin ${CARGO_TARGET_DIR}/release/ckb ${CKB_TEST_ARGS} 2>&1 | tee "${test_log_file}"
   EXIT_CODE=$?
 fi

 if [ $name == "ci_quick_check" ];then
    echo "ci_quick_check"
    make check-cargotoml
    make check-whitespaces
    make check-dirty-rpc-doc
    make check-dirty-hashes-toml
    devtools/ci/check-cyclic-dependencies.py
    EXIT_CODE=$?
 fi
 if [ $name == "ci_linters" ];then
    echo "ci_linters"
    cargo fmt --version ||  rustup component add rustfmt
    cargo clippy --version ||  rustup component add clippy
    make fmt
    make clippy
    git diff --exit-code Cargo.lock
    EXIT_CODE=$?
 fi

 if [ $name == "ci_security_audit_licenses" ];then
    echo "ci_security_audit_licenses"
    cargo deny --version || cargo install cargo-deny --locked
    make security-audit
    make check-crates
    make check-licenses
    EXIT_CODE=$?
 fi

 if [ $name == "ci_WASM_build" ];then
    echo "ci_WASM_build"
    rustup target add wasm32-unknown-unknown
    make wasm-build-test
    EXIT_CODE=$?
 fi
 echo $EXIT_CODE
}

#Get commit sha and message by event name
if [ $EVENT_NAME == "push" ];then
   COMMIT_SHA=$COMMIT_SHA
   MESSAGE="$COMMIT_MESSAGE"
fi

if [ $EVENT_NAME == "pull_request" ];then
    COMMIT_SHA=$PR_COMMIT_SHA
    MESSAGE="$PR_COMMONS_BODY"
fi

nervosnetwork_actor_list='"janx", "doitian", "quake", "xxuejie", "zhangsoledad", "jjyr", "TheWaWaR", "driftluo", "keroro520", "yangby-cryptape","liya2017"'
export EXIT_CODE=0
echo $MESSAGE | grep -q "ci:"
#skip ci
if [[ $? -eq 0 ]]; then
   if [[ $EVENT_NAME == "push" ]]  || [[ $REPO_OWNER == "nervosnetwork" && $EVENT_NAME == "pull_request" &&  $nervosnetwork_actor_list =~ $ACTOR ]];then
      CI_JOB_LIST=`echo $MESSAGE | grep "ci:" | awk -F ":" '{print $2}'`
      if [ $CI_JOB_LIST =~ "unit_tests" ];then
         name="ci_unit_tests"
      fi
      if [ $CI_JOB_LIST =~ "integration_test" ];then
         name="ci_integration_test"
      fi
      if [ $CI_JOB_LIST =~ "benchmark" ];then
         name="ci_benchmarks_test"
      fi
      if [ $CI_JOB_LIST =~ "quick_check" ];then
         name="ci_quick_check"
      fi
      if [ $CI_JOB_LIST =~ "linters" ];then
         name="ci_linters"
      fi
      if [ $CI_JOB_LIST =~ "security_audit" ];then
         name="ci_security_audit_licenses"
      fi
      if [ $CI_JOB_LIST =~ "WASM_build" ];then
         name="ci_WASM_build"
      fi
   fi
else
  name="$1"
fi

func_ci $name

EXIT_STATUS=$EXIT_CODE

set -e
if [ "$EXIT_STATUS" = 0 ]; then
    echo "Check whether the ci succeeds"
else
    echo "Fail the ci"
fi
exit $EXIT_STATUS