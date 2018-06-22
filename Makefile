test:
	cargo test --verbose --all

build:
	cargo build --release

fmt:
	cargo fmt --all -- --check

clippy: 
	cargo clippy --all -- -D warnings -D clone_on_ref_ptr

ci: fmt clippy test
	git diff --exit-code Cargo.lock

proto:
	protoc --rust_out network/protocol/src network/protocol/src/protocol.proto

info:
	date
	pwd
	env

.PHONY: build fmt test clippy ci proto info
