test:
	cargo test --all -- --nocapture

build-integration-test:
	cargo build --all --features integration_test --no-default-features

doc:
	cargo doc --all --no-deps

doc-deps:
	cargo doc --all

check:
	cargo check --all

build:
	cargo build --release

fmt:
	cargo fmt --all -- --check

clippy:
	cargo clippy --all -- -D warnings -D clone_on_ref_ptr -D unused_extern_crates -D enum_glob_use

ci: fmt clippy test build-integration-test
	git diff --exit-code Cargo.lock

ci-quick: test build-integration-test
	git diff --exit-code Cargo.lock

info:
	date
	pwd
	env

.PHONY: build build-integration-test
.PHONY: fmt test clippy proto doc doc-deps check
.PHONY: ci ci-quick info
