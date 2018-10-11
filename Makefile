test:
	RUSTFLAGS="--cfg ckb_test" cargo test --all -- --nocapture

build-integration-test:
	RUSTFLAGS="--cfg ckb_test" cargo build --all --features integration_test --no-default-features

doc:
	RUSTFLAGS="--cfg ckb_test" cargo doc --all --no-deps

doc-deps:
	RUSTFLAGS="--cfg ckb_test" cargo doc --all

check:
	RUSTFLAGS="--cfg ckb_test" cargo check --all

build:
	cargo build --release

fmt:
	cargo fmt --all -- --check

clippy:
	RUSTFLAGS="--cfg ckb_test" cargo clippy --all -- -D warnings -D clone_on_ref_ptr -D unused_extern_crates -D enum_glob_use

ci: fmt clippy test build-integration-test
	git diff --exit-code Cargo.lock

ci-quick: test build-integration-test
	git diff --exit-code Cargo.lock

proto:
	protoc --rust_out network/protocol/src network/protocol/src/protocol.proto

info:
	date
	pwd
	env

.PHONY: build build-integration-test
.PHONY: fmt test clippy proto doc doc-deps check
.PHONY: ci ci-quick info
