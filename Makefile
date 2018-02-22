test:
	cargo test --release

build:
	cargo build --release

fmt:
	cargo fmt --all -- --write-mode=diff

ci: fmt test
	git diff --exit-code Cargo.lock

.PHONY: build fmt test ci
