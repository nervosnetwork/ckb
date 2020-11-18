.DEFAULT_GOAL:=help
SHELL = /bin/sh
MOLC    := moleculec
MOLC_VERSION := 0.6.0
VERBOSE := $(if ${CI},--verbose,)
CLIPPY_OPTS := -D warnings -D clippy::clone_on_ref_ptr -D clippy::enum_glob_use -D clippy::fallible_impl_from \
	-A clippy::mutable_key_type
CKB_TEST_ARGS := -c 4
INTEGRATION_RUST_LOG := ckb-network=error

##@ Testing
.PHONY: test
test: ## Run all tests.
	cargo test ${VERBOSE} --all -- --nocapture

# Tarpaulin only supports x86_64 processors running Linux.
# https://github.com/xd009642/tarpaulin/issues/161
# https://github.com/xd009642/tarpaulin/issues/190#issuecomment-473564880
.PHONY: cov
cov: ## Run code coverage.
	RUSTC="$$(pwd)/devtools/cov/rustc-proptest-fix" taskset -c 0 cargo tarpaulin --timeout 300 --exclude-files "*/generated/" "test/*" "*/tests/" --all -v --out Xml

.PHONY: wasm-build-test
wasm-build-test: ## Build core packages for wasm target
	cd wasm-build-test && cargo build --target=wasm32-unknown-unknown

.PHONY: setup-ckb-test
setup-ckb-test:
	cp -f Cargo.lock test/Cargo.lock
	rm -rf test/target && ln -snf ../target/ test/target

.PHONY: submodule-init
submodule-init:
	git submodule update --init

.PHONY: integration
integration: submodule-init setup-ckb-test ## Run integration tests in "test" dir.
	cargo build --features deadlock_detection
	RUST_BACKTRACE=1 RUST_LOG=${INTEGRATION_RUST_LOG} test/run.sh -- --bin ../target/debug/ckb ${CKB_TEST_ARGS}

.PHONY: integration-release
integration-release: submodule-init setup-ckb-test
	cargo build --release --features deadlock_detection
	RUST_BACKTRACE=1 RUST_LOG=${INTEGRATION_RUST_LOG} test/run.sh --release -- --bin ../target/release/ckb ${CKB_TEST_ARGS}

##@ Document
.PHONY: doc
doc: ## Build the documentation for the local package.
	cargo doc --all --no-deps

.PHONY: doc-deps
doc-deps: ## Build the documentation for the local package and all dependencies.
	cargo doc --all

.PHONY: gen-rpc-doc
gen-rpc-doc:  ## Generate rpc documentation
	rm -f target/doc/ckb_rpc/module/trait.*.html
	cargo doc -p ckb-rpc -p ckb-types -p ckb-fixed-hash -p ckb-fixed-hash-core -p ckb-jsonrpc-types --no-deps
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
	cargo check ${VERBOSE} --all --all-targets --all-features
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
	RUSTFLAGS="--cfg disable_faketime" cargo build ${VERBOSE} --release

.PHONY: prod-docker
prod-docker:
	RUSTFLAGS="--cfg disable_faketime --cfg docker" cargo build --verbose --release

.PHONY: prod-test
prod-test:
	RUSTFLAGS="--cfg disable_faketime" RUSTDOCFLAGS="--cfg disable_faketime" cargo test ${VERBOSE} --all -- --nocapture

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
	cargo clippy ${VERBOSE} --all --all-targets --all-features -- ${CLIPPY_OPTS} -D missing_docs
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
	cd benches && cargo bench --features ci -- --test

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

${GEN_MOL_OUT_DIR}/blockchain.rs: ${GEN_MOL_IN_DIR}/blockchain.mol
	${MOLC} --language rust --schema-file $< | rustfmt > $@

${GEN_MOL_OUT_DIR}/extensions.rs: ${GEN_MOL_IN_DIR}/extensions.mol
	${MOLC} --language rust --schema-file $< | rustfmt > $@

${GEN_MOL_OUT_DIR}/protocols.rs: ${GEN_MOL_IN_DIR}/protocols.mol
	${MOLC} --language rust --schema-file $< | rustfmt > $@

##@ Cleanup
.PHONY: clean-node-files
clean-node-files: ## Clean files generated by `ckb init`
	rm -rf ckb.toml ckb-miner.toml specs/ data/

##@ Helpers
.PHONY: stats
stats: ## Counting lines of code.
	@cargo count --version || cargo +nightly install --git https://github.com/kbknapp/cargo-count
	@cargo count --separator , --unsafe-statistics

.PHONY: help
help:  ## Display help message.
	@awk 'BEGIN {FS = ":.*##"; printf "\nUsage:\n  make \033[36m<target>\033[0m\n"} /^[a-zA-Z_-]+:.*?##/ { printf "  \033[36m%-15s\033[0m %s\n", $$1, $$2 } /^##@/ { printf "\n\033[1m%s\033[0m\n", substr($$0, 5) } ' $(MAKEFILE_LIST)
