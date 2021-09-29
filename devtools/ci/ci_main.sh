#!/bin/bash
set -u
is_self_runner=`echo $RUNNER_LABEL | awk -F '-' '{print $1}'`
if [[ $is_self_runner == "self" ]];then
  CARGO_TARGET_DIR=$GITHUB_WORKSPACE/../target
fi
CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-"$GITHUB_WORKSPACE/target"}
EXIT_CODE=0
CLIPPY_OPTS='-D warnings -D clippy::clone_on_ref_ptr -D clippy::enum_glob_use -D clippy::fallible_impl_from -A clippy::mutable_key_type -A clippy::upper_case_acronyms'
case $GITHUB_WORKFLOW in
  ci_linters*)
    echo "ci_linters"
    cargo fmt --version ||  rustup component add rustfmt
    if [ $? -eq 0 ]; then
	     printf "SUCCESS\n"
    else
       printf "FAILURE\n"
	     EXIT_CODE=1
    fi

    cargo clippy --version ||  rustup component add clippy
    if [ $? -eq 0 ]; then
	     printf "SUCCESS\n"
    else
       printf "FAILURE\n"
	     EXIT_CODE=1
    fi

    cp -f Cargo.lock test/Cargo.lock
    rm -rf test/target && ln -snf ${CARGO_TARGET_DIR} test/target
    cargo fmt --verbose --all -- --check
    if [ $? -eq 0 ]; then
	     printf "SUCCESS\n"
    else
       printf "FAILURE\n"
	     EXIT_CODE=1
    fi

    cargo clippy --verbose --all --all-targets --all-features -- ${CLIPPY_OPTS} -D missing_docs
    if [ $? -eq 0 ]; then
	     printf "SUCCESS\n"
    else
	     printf "FAILURE\n"
	     EXIT_CODE=1
    fi

    cd test
    cargo fmt --verbose --all -- --check
    if [ $? -eq 0 ]; then
	     printf "SUCCESS\n"
    else
	     printf "FAILURE\n"
	     EXIT_CODE=1
    fi

    cargo clippy --verbose --all --all-targets --all-features -- ${CLIPPY_OPTS}
    if [ $? -eq 0 ]; then
	     printf "SUCCESS\n"
    else
	     printf "FAILURE\n"
	     EXIT_CODE=1
    fi
    cd ../
    git diff --exit-code Cargo.lock
    if [ $? -eq 0 ]; then
	     printf "SUCCESS\n"
    else
	     printf "FAILURE\n"
	     EXIT_CODE=1
    fi
    ;;
  ci_unit_test*)
    echo "ci_unit_tests"
    make test
    if [ $? -eq 0 ]; then
	     printf "SUCCESS\n"
    else
	     printf "FAILURE\n"
	     EXIT_CODE=1
    fi
    ;;
  ci_benchmarks*)
    echo "ci_benchmarks_test"
    make bench-test
    if [ $? -eq 0 ]; then
	     printf "SUCCESS\n"
    else
	     printf "FAILURE\n"
	     EXIT_CODE=1
    fi
    ;;
ci_integration_tests*)
    echo "ci_integration_test"
    github_workflow_os=`echo $GITHUB_WORKFLOW | awk -F '_' '{print $NF}'`
    make submodule-init
    if [ $? -eq 0 ]; then
	     printf "SUCCESS\n"
    else
	     printf "FAILURE\n"
	     EXIT_CODE=1
    fi
    cp -f Cargo.lock test/Cargo.lock
    rm -rf test/target && ln -snf ${CARGO_TARGET_DIR} test/target
    cargo build --release --features deadlock_detection --target-dir $CARGO_TARGET_DIR
    if [ $? -eq 0 ]; then
	     printf "SUCCESS\n"
    else
	     printf "FAILURE\n"
	     EXIT_CODE=1
    fi
    git diff --exit-code Cargo.lock
    if [ $? -eq 0 ]; then
	     printf "SUCCESS\n"
    else
	     printf "FAILURE\n"
	     EXIT_CODE=1
    fi
    cd test
    if [[ $github_workflow_os == 'windows' ]];then
      cargo run -- --bin ${CARGO_TARGET_DIR}/release/ckb.exe --log-file ${GITHUB_WORKSPACE}/integration.log ${CKB_TEST_ARGS}
      if [ $? -eq 0 ]; then
	       printf "SUCCESS\n"
      else
	       printf "FAILURE\n"
	       EXIT_CODE=1
      fi
    else
      cargo run -- --bin ${CARGO_TARGET_DIR}/release/ckb --log-file ${GITHUB_WORKSPACE}/integration.log ${CKB_TEST_ARGS}
      if [ $? -eq 0 ]; then
	       printf "SUCCESS\n"
      else
	       printf "FAILURE\n"
	       EXIT_CODE=1
      fi
    fi
    ;;
  ci_quick_checks*)
    echo "ci_quick_check"
    make check-cargotoml
    if [ $? -eq 0 ]; then
	     printf "SUCCESS\n"
    else
	     printf "FAILURE\n"
	     EXIT_CODE=1
    fi
    make check-whitespaces
    if [ $? -eq 0 ]; then
	     printf "SUCCESS\n"
    else
	     printf "FAILURE\n"
	     EXIT_CODE=1
    fi
    make check-dirty-rpc-doc
    if [ $? -eq 0 ]; then
	     printf "SUCCESS\n"
    else
	     printf "FAILURE\n"
	     EXIT_CODE=1
    fi
    make check-dirty-hashes-toml
    if [ $? -eq 0 ]; then
	     printf "SUCCESS\n"
    else
	     printf "FAILURE\n"
	     EXIT_CODE=1
    fi
    devtools/ci/check-cyclic-dependencies.py
    if [ $? -eq 0 ]; then
	     printf "SUCCESS\n"
    else
	     printf "FAILURE\n"
	     EXIT_CODE=1
    fi
    ;;
  ci_wasm_build*)
    echo "ci_WASM_build"
    rustup target add wasm32-unknown-unknown
    if [ $? -eq 0 ]; then
	     printf "SUCCESS\n"
    else
	     printf "FAILURE\n"
	     EXIT_CODE=1
    fi
    make wasm-build-test
    if [ $? -eq 0 ]; then
	     printf "SUCCESS\n"
    else
	     printf "FAILURE\n"
	     EXIT_CODE=1
    fi
    ;;
  ci_cargo_deny*)
    echo "ci_security_audit_licenses"
    cargo deny --version || cargo install cargo-deny --locked
    if [ $? -eq 0 ]; then
	     printf "SUCCESS\n"
    else
	     printf "FAILURE\n"
	     EXIT_CODE=1
    fi
    make security-audit
    if [ $? -eq 0 ]; then
	     printf "SUCCESS\n"
    else
	     printf "FAILURE\n"
	     EXIT_CODE=1
    fi
    make check-crates
    if [ $? -eq 0 ]; then
	     printf "SUCCESS\n"
    else
	     printf "FAILURE\n"
	     EXIT_CODE=1
    fi
    make check-licenses
    if [ $? -eq 0 ]; then
	     printf "SUCCESS\n"
    else
	     printf "FAILURE\n"
	     EXIT_CODE=1
    fi
    ;;
  *)
    echo -n "unknown"
    ;;
esac
echo " EXIT_CODE is "$EXIT_CODE
exit $EXIT_CODE