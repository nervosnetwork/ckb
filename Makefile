test:
	cargo test --verbose --all

build:
	cargo build --release

fmt:
	cargo fmt --all -- --write-mode=diff

clippy: 
	cargo clippy --all -- -D warnings -D clone_on_ref_ptr

ci: fmt clippy test
	git diff --exit-code Cargo.lock

.PHONY: build fmt test clippy ci
