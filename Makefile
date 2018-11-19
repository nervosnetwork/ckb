test:
	cargo test --verbose --all

build:
	cargo build --release

fmt:
	cargo fmt --all -- --write-mode=diff

clippy: 
	cargo clippy --all -- -D warnings

ci: fmt clippy test
	git diff --exit-code Cargo.lock

.PHONY: build fmt test clippy ci
