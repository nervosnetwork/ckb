.DEFAULT_GOAL:=help
SHELL = /bin/sh
FLATC   := flatc
CFBC    := cfbc
VERBOSE := $(if ${CI},--verbose,)

##@ Testing
test: ## Run all tests.
	cargo test ${VERBOSE} --all -- --nocapture

setup-ckb-test:
	cp -f Cargo.lock test/Cargo.lock
	rm -rf test/target && ln -snf ../target/ test/target

integration: setup-ckb-test ## Run integration tests in "test" dir.
	cargo build ${VERBOSE}
	cd test && cargo run ../target/debug/ckb

integration-release: setup-ckb-test ## Run integration tests in "test" dir with release build.
	cargo build ${VERBOSE} --release
	cd test && cargo run --release -- ../target/release/ckb

##@ Document
doc: ## Build the documentation for the local package.
	cargo doc --all --no-deps

doc-deps: ## Build the documentation for the local package and all dependencies.
	cargo doc --all

##@ Building
check: setup-ckb-test ## Runs all of the compiler's checks.
	cargo check ${VERBOSE} --all
	cd test && cargo check ${VERBOSE} --all

build: ## Build binary with release profile.
	cargo build ${VERBOSE} --release

prod: ## Build binary for production release.
	RUSTFLAGS="--cfg disable_faketime" cargo build ${VERBOSE} --release

prod-test: ## Build binary for testing production release.
	RUSTFLAGS="--cfg disable_faketime" RUSTDOCFLAGS="--cfg disable_faketime" cargo test ${VERBOSE} --all -- --nocapture

docker: ## Build docker image with the bin built from "prod" then push it to Docker Hub as nervos/ckb:latest .
	docker build -f docker/hub/Dockerfile -t nervos/ckb:latest .

##@ Code Quality
fmt: setup-ckb-test ## Check Rust source code format to keep to the same style.
	cargo fmt ${VERBOSE} --all -- --check
	cd test && cargo fmt ${VERBOSE} --all -- --check

clippy: setup-ckb-test ## Run linter to examine Rust source codes.
	cargo clippy ${VERBOSE} --all --all-targets --all-features -- -D warnings -D clippy::clone_on_ref_ptr -D clippy::enum_glob_use -D clippy::fallible_impl_from
	cd test && cargo clippy ${VERBOSE} --all --all-targets --all-features -- -D warnings -D clippy::clone_on_ref_ptr -D clippy::enum_glob_use -D clippy::fallible_impl_from

security-audit: ## Use cargo-audit to audit Cargo.lock for crates with security vulnerabilities.
	@cargo audit --version || cargo install cargo-audit
	@cargo audit
	# expecting to see "Success No vulnerable packages found"

##@ Continuous Integration

ci: ## Run recipes for CI.
ci: fmt clippy security-audit test
	git diff --exit-code Cargo.lock

info: ## Show environment info.
	date
	pwd
	env

##@ Generates Files
GEN_FILES := protocol/src/protocol_generated.rs protocol/src/protocol_generated_verifier.rs
gen: ${GEN_FILES}
gen-clean:
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
clean: ## Clean tmp files.
	rm -rf ckb.toml ckb-miner.toml specs/

##@ Helpers

stats: ## Counting lines of code.
	@cargo count --version || cargo +nightly install --git https://github.com/kbknapp/cargo-count
	@cargo count --separator , --unsafe-statistics

help:  ## Display help message.
	@awk 'BEGIN {FS = ":.*##"; printf "\nUsage:\n  make \033[36m<target>\033[0m\n"} /^[a-zA-Z_-]+:.*?##/ { printf "  \033[36m%-15s\033[0m %s\n", $$1, $$2 } /^##@/ { printf "\n\033[1m%s\033[0m\n", substr($$0, 5) } ' $(MAKEFILE_LIST)

.PHONY: build prod prod-test docker
.PHONY: gen gen-clean clean check-cfbc-version
.PHONY: fmt test clippy doc doc-deps check stats
.PHONY: ci info security-audit
.PHONY: integration integration-release setup-ckb-test
