.DEFAULT_GOAL:=help
SHELL = /bin/sh
MOLC    := moleculec
MOLC_VERSION := 0.7.5
VERBOSE := $(if ${CI},--verbose,)
CLIPPY_OPTS := -D warnings -D clippy::clone_on_ref_ptr -D clippy::enum_glob_use -D clippy::fallible_impl_from \
	-A clippy::mutable_key_type -A clippy::upper_case_acronyms
CKB_TEST_ARGS := ${CKB_TEST_ARGS} -c 4
CKB_FEATURES ?= deadlock_detection,with_sentry
ALL_FEATURES := deadlock_detection,with_sentry,with_dns_seeding,profiling,march-native
CKB_BENCH_FEATURES ?= ci
INTEGRATION_RUST_LOG := info,ckb_test=debug,ckb_sync=debug,ckb_relay=debug,ckb_network=debug
CARGO_TARGET_DIR ?= $(shell pwd)/target
BINARY_NAME ?= "ckb"
COV_PROFRAW_DIR = ${CARGO_TARGET_DIR}/cov
GRCOV_OUTPUT ?= lcov.info
GRCOV_EXCL_START = ^\s*(((log|ckg_logger)::)?(trace|debug|info|warn|error)|(debug_)?assert(_eq|_ne|_error_eq))!\($$
GRCOV_EXCL_STOP  = ^\s*\)(;)?$$
GRCOV_EXCL_LINE = \s*(((log|ckg_logger)::)?(trace|debug|info|warn|error)|(debug_)?assert(_eq|_ne|_error_eq))!\(.*\)(;)?$$

##@ Testing
.PHONY: doc-test
doc-test: ## Run doc tests
	cargo test --all --doc

.PHONY: cli-test
cli-test: build # Run ckb command line usage bats test
	./util/app-config/src/tests/cli_test.sh

.PHONY: test
test: ## Run all tests, including some tests can be time-consuming to execute (tagged with [ignore])
	cargo nextest run ${VERBOSE} --features ${CKB_FEATURES} --workspace --hide-progress-bar --success-output immediate-final --failure-output immediate-final --run-ignored all
	$(MAKE) doc-test

.PHONY: quick-test
quick-test: ## Run all tests, excluding some tests can be time-consuming to execute (tagged with [ignore])
	cargo nextest run ${VERBOSE} --features ${CKB_FEATURES} --workspace --hide-progress-bar --success-output immediate-final --failure-output immediate-final --run-ignored default
	$(MAKE) doc-test

.PHONY: cov-install-tools
cov-install-tools:
	rustup component add llvm-tools-preview --toolchain nightly-2022-03-22
	grcov --version || cargo +nightly-2022-03-22 install grcov

.PHONY: cov-collect-data
cov-collect-data:
	RUSTUP_TOOLCHAIN=nightly-2022-03-22 \
	grcov "${COV_PROFRAW_DIR}" --binary-path "${CARGO_TARGET_DIR}/debug/" \
		-s . -t lcov --branch --ignore-not-existing --ignore "/*" \
		--ignore "*/tests/*" \
		--ignore "*/tests.rs" \
		--ignore "*/generated/*" \
		--excl-br-start "${GRCOV_EXCL_START}" --excl-br-stop "${GRCOV_EXCL_STOP}" \
		--excl-start    "${GRCOV_EXCL_START}" --excl-stop    "${GRCOV_EXCL_STOP}" \
		--excl-br-line  "${GRCOV_EXCL_LINE}" \
		--excl-line     "${GRCOV_EXCL_LINE}" \
		-o "${GRCOV_OUTPUT}"

.PHONY: cov-gen-report
cov-gen-report:
	genhtml -o "$(GRCOV_OUTPUT:.info=)" "${GRCOV_OUTPUT}"

.PHONY: cov
cov: cov-install-tools ## Run code coverage.
	mkdir -p "${COV_PROFRAW_DIR}"; rm -f "${COV_PROFRAW_DIR}/*.profraw"
	RUSTFLAGS="-Zinstrument-coverage" LLVM_PROFILE_FILE="${COV_PROFRAW_DIR}/ckb-cov-%p-%m.profraw" cargo +nightly-2022-03-22 test --all
	GRCOV_OUTPUT=lcov-unit-test.info make cov-collect-data

.PHONY: wasm-build-test
wasm-build-test: ## Build core packages for wasm target
	cp -f Cargo.lock wasm-build-test/
	cd wasm-build-test && cargo build --target=wasm32-unknown-unknown

.PHONY: setup-ckb-test
setup-ckb-test:
	cp -f Cargo.lock test/Cargo.lock

.PHONY: submodule-init
submodule-init:
	git submodule update --init

.PHONY: integration
integration: submodule-init setup-ckb-test ## Run integration tests in "test" dir.
	cargo build --release --features ${CKB_FEATURES}
	RUST_BACKTRACE=1 RUST_LOG=${INTEGRATION_RUST_LOG} test/run.sh -- --bin "${CARGO_TARGET_DIR}/release/${BINARY_NAME}" ${CKB_TEST_ARGS}

.PHONY: integration-release
integration-release: submodule-init setup-ckb-test build
	RUST_BACKTRACE=1 RUST_LOG=${INTEGRATION_RUST_LOG} test/run.sh -- --bin ${CARGO_TARGET_DIR}/release/ckb ${CKB_TEST_ARGS}

.PHONY: integration-cov
integration-cov: cov-install-tools submodule-init setup-ckb-test ## Run integration tests and genearte coverage report.
	mkdir -p "${COV_PROFRAW_DIR}"; rm -f "${COV_PROFRAW_DIR}/*.profraw"
	RUSTFLAGS="-Zinstrument-coverage" LLVM_PROFILE_FILE="${COV_PROFRAW_DIR}/ckb-cov-%p-%m.profraw" cargo +nightly-2022-03-22 build --features deadlock_detection
	RUST_BACKTRACE=1 RUST_LOG=${INTEGRATION_RUST_LOG} test/run.sh -- --bin ${CARGO_TARGET_DIR}/debug/ckb ${CKB_TEST_ARGS}
	GRCOV_OUTPUT=lcov-integration-test.info make cov-collect-data

##@ Document
.PHONY: doc
doc: ## Build the documentation for the local package.
	cargo doc --workspace --no-deps

.PHONY: doc-deps
doc-deps: ## Build the documentation for the local package and all dependencies.
	cargo doc --workspace

.PHONY: gen-rpc-doc
gen-rpc-doc:  ## Generate rpc documentation
	rm -f ${CARGO_TARGET_DIR}/doc/ckb_rpc/module/trait.*.html
	cargo doc --workspace
	ln -nsf "${CARGO_TARGET_DIR}" "target"
	if command -v python3 &> /dev/null; then \
		python3 ./devtools/doc/rpc.py > rpc/README.md; \
	else \
		python ./devtools/doc/rpc.py > rpc/README.md; \
	fi

.PHONY: gen-hashes
gen-hashes: ## Generate docs/hashes.toml
	cargo run list-hashes -b > docs/hashes.toml

##@ Building
.PHONY: check
check: setup-ckb-test ## Runs all of the compiler's checks.
	cargo check ${VERBOSE} --all --all-targets --features ${ALL_FEATURES}
	cd test && cargo check ${VERBOSE} --all --all-targets --all-features

.PHONY: build
build: ## Build binary with release profile.
	cargo build ${VERBOSE} --release

.PHONY: build-for-profiling-without-debug-symbols
build-for-profiling-without-debug-symbols: ## Build binary with for profiling without debug symbols.
	JEMALLOC_SYS_WITH_MALLOC_CONF="prof:true" cargo build ${VERBOSE} --release --features "profiling"

.PHONY: build-for-profiling
build-for-profiling: ## Build binary with for profiling.
	devtools/release/make-with-debug-symbols build-for-profiling-without-debug-symbols

.PHONY: prod
prod: ## Build binary for production release.
	RUSTFLAGS="$${RUSTFLAGS} --cfg disable_faketime" cargo build ${VERBOSE} --profile prod --features "with_sentry,with_dns_seeding"

.PHONY: prod_portable
prod_portable: ## Build binary for portable production release.
	RUSTFLAGS="$${RUSTFLAGS} --cfg disable_faketime" cargo build ${VERBOSE} --profile prod --features "with_sentry,with_dns_seeding,portable"

.PHONY: prod-docker
prod-docker:
	RUSTFLAGS="$${RUSTFLAGS} --cfg disable_faketime --cfg docker" cargo build --verbose --profile prod --features "with_sentry,with_dns_seeding"

.PHONY: prod-test
prod-test:
	RUSTFLAGS="$${RUSTFLAGS} --cfg disable_faketime" RUSTDOCFLAGS="--cfg disable_faketime" CKB_FEATURES="with_sentry,with_dns_seeding" $(MAKE) test

.PHONY: prod-with-debug
prod-with-debug:
	devtools/release/make-with-debug-symbols prod

.PHONY: docker
docker: ## Build docker image
	docker build -f docker/hub/Dockerfile -t nervos/ckb:$$(git describe) .
	docker run --rm -it nervos/ckb:$$(git describe) --version

.PHONY: docker-publish
docker-publish:
	docker push nervos/ckb:$$(git describe)
	docker tag nervos/ckb:$$(git describe) nervos/ckb:latest
	docker push nervos/ckb:latest

##@ Code Quality
.PHONY: fmt
fmt: setup-ckb-test ## Check Rust source code format to keep to the same style.
	cargo fmt ${VERBOSE} --all -- --check
	cd test && cargo fmt ${VERBOSE} --all -- --check

.PHONY: clippy
clippy: setup-ckb-test ## Run linter to examine Rust source codes.
	cargo clippy ${VERBOSE} --all --all-targets --features ${ALL_FEATURES} -- ${CLIPPY_OPTS} -D missing_docs
	cd test && cargo clippy ${VERBOSE} --all --all-targets --all-features -- ${CLIPPY_OPTS}

.PHONY: security-audit
security-audit: ## Use cargo-deny to audit Cargo.lock for crates with security vulnerabilities.
	cargo deny check --hide-inclusion-graph --show-stats advisories sources

.PHONY: check-crates
check-crates: ## Use cargo-deny to check specific crates, detect and handle multiple versions of the same crate and wildcards version requirement.
	cargo deny check --hide-inclusion-graph --show-stats bans

.PHONY: check-licenses
check-licenses: ## Use cargo-deny to check licenses for all dependencies.
	cargo deny check --hide-inclusion-graph --show-stats licenses

.PHONY: bench-test
bench-test:
	cd benches && cargo bench --features ${CKB_BENCH_FEATURES} -- --test

##@ Continuous Integration

.PHONY: ci
ci: ## Run recipes for CI.
ci: fmt clippy test bench-test check-cargotoml check-whitespaces check-dirty-rpc-doc security-audit check-crates check-licenses
	git diff --exit-code Cargo.lock

.PHONY: check-cargotoml
check-cargotoml:
	./devtools/ci/check-cargotoml.sh

.PHONY: check-whitespace
check-whitespaces:
	git -c core.whitespace=-blank-at-eof diff-index --check --cached $$(git rev-parse --verify master 2>/dev/null || echo "4b825dc642cb6eb9a060e54bf8d69288fbee4904") --

.PHONY: check-dirty-rpc-doc
check-dirty-rpc-doc: gen-rpc-doc
	git diff --exit-code rpc/README.md

.PHONY: check-dirty-hashes-toml
check-dirty-hashes-toml: gen-hashes
	git diff --exit-code docs/hashes.toml

##@ Generates Files
.PHONY: gen
GEN_MOL_IN_DIR := util/types/schemas
GEN_MOL_OUT_DIR := util/types/src/generated
GEN_MOL_FILES := ${GEN_MOL_OUT_DIR}/blockchain.rs ${GEN_MOL_OUT_DIR}/extensions.rs ${GEN_MOL_OUT_DIR}/protocols.rs
gen: check-moleculec-version ${GEN_MOL_FILES} # Generate Protocol Files

.PHONY: check-moleculec-version
check-moleculec-version:
	test "$$(${MOLC} --version | awk '{ print $$2 }' | tr -d ' ')" = ${MOLC_VERSION}

.PHONY: ${GEN_MOL_OUT_DIR}/blockchain.rs
${GEN_MOL_OUT_DIR}/blockchain.rs: ${GEN_MOL_IN_DIR}/blockchain.mol
	${MOLC} --language rust --schema-file $< | rustfmt > $@

.PHONY: ${GEN_MOL_OUT_DIR}/extensions.rs
${GEN_MOL_OUT_DIR}/extensions.rs: ${GEN_MOL_IN_DIR}/extensions.mol
	${MOLC} --language rust --schema-file $< | rustfmt > $@

.PHONY: ${GEN_MOL_OUT_DIR}/protocols.rs
${GEN_MOL_OUT_DIR}/protocols.rs: ${GEN_MOL_IN_DIR}/protocols.mol
	${MOLC} --language rust --schema-file $< | rustfmt > $@

##@ Cleanup
.PHONY: clean-node-files
clean-node-files: ## Clean files generated by `ckb init`
	rm -rf ckb.toml ckb-miner.toml default.db-options specs/ data/

##@ Helpers
.PHONY: stats
stats: ## Counting lines of code.
	@cargo count --version || cargo +nightly-2022-03-22 install --git https://github.com/kbknapp/cargo-count
	@cargo count --separator , --unsafe-statistics

.PHONY: help
help:  ## Display help message.
	@awk 'BEGIN {FS = ":.*##"; printf "\nUsage:\n  make \033[36m<target>\033[0m\n"} /^[a-zA-Z_-]+:.*?##/ { printf "  \033[36m%-15s\033[0m %s\n", $$1, $$2 } /^##@/ { printf "\n\033[1m%s\033[0m\n", substr($$0, 5) } ' $(MAKEFILE_LIST)
