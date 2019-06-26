.DEFAULT_GOAL:=help
SHELL = /bin/sh
FLATC   := flatc
CFBC    := cfbc
VERBOSE := $(if ${CI},--verbose,)
CLIPPY_OPTS := -D warnings -D clippy::clone_on_ref_ptr -D clippy::enum_glob_use -D clippy::fallible_impl_from

##@ Testing
test: ## Run all tests.
	cargo test ${VERBOSE} --all -- --nocapture

cov: ## Run code coverage.
	# Tarpaulin only supports x86_64 processors running Linux.
	# https://github.com/xd009642/tarpaulin/issues/161
	# https://github.com/xd009642/tarpaulin/issues/190#issuecomment-473564880
	RUSTC="$$(pwd)/devtools/cov/rustc-proptest-fix" taskset -c 0 cargo tarpaulin --exclude-files protocol/src/protocol_generated* test/* */tests/ --all -v --out Xml

setup-ckb-test:
	cp -f Cargo.lock test/Cargo.lock
	rm -rf test/target && ln -snf ../target/ test/target

integration: setup-ckb-test ## Run integration tests in "test" dir.
	cargo build ${VERBOSE}
	cd test && cargo run ${VERBOSE} -- ../target/debug/ckb

integration-windows:
	cp -f Cargo.lock test/Cargo.lock
	cargo build ${VERBOSE}
	mv target test/
	cd test && cargo run ${VERBOSE} -- target/debug/ckb

integration-release: setup-ckb-test
	cargo build ${VERBOSE} --release
	cd test && cargo run ${VERBOSE} --release -- ../target/release/ckb

##@ Document
doc: ## Build the documentation for the local package.
	cargo doc --all --no-deps

doc-deps: ## Build the documentation for the local package and all dependencies.
	cargo doc --all

gen-doc:  ## Generate rpc documentation
	./devtools/doc/jsonfmt.py rpc/json/rpc.json
	./devtools/doc/rpc.py rpc/json/rpc.json > rpc/README.md

gen-hashes: ## Generate docs/hashes.toml
	cargo run cli hashes -b > docs/hashes.toml

##@ Building
check: setup-ckb-test ## Runs all of the compiler's checks.
	cargo check ${VERBOSE} --all
	cd test && cargo check ${VERBOSE} --all

build: ## Build binary with release profile.
	cargo build ${VERBOSE} --release

prod: ## Build binary for production release.
	RUSTFLAGS="--cfg disable_faketime" cargo build ${VERBOSE} --release

prod-docker:
	RUSTFLAGS="--cfg disable_faketime --cfg docker" cargo build --verbose --release

prod-test:
	RUSTFLAGS="--cfg disable_faketime" RUSTDOCFLAGS="--cfg disable_faketime" cargo test ${VERBOSE} --all -- --nocapture

docker: ## Build docker image
	docker build -f docker/hub/Dockerfile -t nervos/ckb:$$(git describe) .
	docker run --rm -it nervos/ckb:$$(git describe) --version

docker-publish:
	docker push nervos/ckb:$$(git describe)
	docker tag nervos/ckb:$$(git describe) nervos/ckb:latest
	docker push nervos/ckb:latest

##@ Code Quality
fmt: setup-ckb-test ## Check Rust source code format to keep to the same style.
	cargo fmt ${VERBOSE} --all -- --check
	cd test && cargo fmt ${VERBOSE} --all -- --check

clippy: setup-ckb-test ## Run linter to examine Rust source codes.
	cargo clippy ${VERBOSE} --all --all-targets --all-features -- ${CLIPPY_OPTS}
	cd test && cargo clippy ${VERBOSE} --all --all-targets --all-features -- ${CLIPPY_OPTS}

security-audit: ## Use cargo-audit to audit Cargo.lock for crates with security vulnerabilities.
	@cargo audit --version || cargo install cargo-audit
	@cargo audit
	# expecting to see "Success No vulnerable packages found"

##@ Continuous Integration

ci: ## Run recipes for CI.
ci: check-cargotoml fmt check-dirty-doc clippy security-audit test
	git diff --exit-code Cargo.lock

check-cargotoml:
	./devtools/ci/check-cargotoml.sh

check-dirty-doc: gen-doc
	git diff --exit-code rpc/README.md rpc/json/rpc.json

##@ Generates Files
GEN_FILES := protocol/src/protocol_generated.rs protocol/src/protocol_generated_verifier.rs
gen: ${GEN_FILES} # Generate Protocol Files
gen-clean: # Clean Protocol Failes
	rm -f ${GEN_FILES}

check-cfbc-version:
	test "$$($(CFBC) --version)" = 0.1.9

%_generated.rs: %.fbs
	$(FLATC) -r -o $(shell dirname $@) $<

%_generated_verifier.rs: %.fbs check-cfbc-version
	$(FLATC) -b --schema -o $(shell dirname $@) $<
	$(CFBC) -o $(shell dirname $@) $*.bfbs
	rm -f $*.bfbs $*_builder.rs

##@ Cleanup
clean: ## Clean files generated by `ckb init`
	rm -rf ckb.toml ckb-miner.toml specs/
clean-all: ## Clean files generated by `ckb init` and data directory
	rm -rf ckb.toml ckb-miner.toml specs/ data/

##@ Helpers

stats: ## Counting lines of code.
	@cargo count --version || cargo +nightly install --git https://github.com/kbknapp/cargo-count
	@cargo count --separator , --unsafe-statistics

help:  ## Display help message.
	@awk 'BEGIN {FS = ":.*##"; printf "\nUsage:\n  make \033[36m<target>\033[0m\n"} /^[a-zA-Z_-]+:.*?##/ { printf "  \033[36m%-15s\033[0m %s\n", $$1, $$2 } /^##@/ { printf "\n\033[1m%s\033[0m\n", substr($$0, 5) } ' $(MAKEFILE_LIST)

.PHONY: build prod prod-test prod-docker docker docker-publish
.PHONY: gen gen-clean clean clean-all check-cfbc-version
.PHONY: fmt test clippy doc doc-deps gen-doc gen-hashes check stats check-dirty-doc cov
.PHONY: ci security-audit
.PHONY: integration integration-release setup-ckb-test
