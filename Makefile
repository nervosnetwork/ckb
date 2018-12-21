test:
	cargo test --all -- --nocapture

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
	cargo clippy --all -- -D warnings -D clippy::clone_on_ref_ptr -D clippy::enum_glob_use

ci: fmt clippy test
	git diff --exit-code Cargo.lock

ci-quick: test
	git diff --exit-code Cargo.lock

info:
	date
	pwd
	env

# For counting lines of code
stats:
	@cargo count --version || cargo +nightly install --git https://github.com/kbknapp/cargo-count
	@cargo count --separator , --unsafe-statistics

# Use cargo-audit to audit Cargo.lock for crates with security vulnerabilities
# expecting to see "Success No vulnerable packages found"
security-audit:
	@cargo audit --version || cargo install cargo-audit
	@cargo audit

docker: build
	docker build -f docker/hub/Dockerfile -t nervos/ckb:latest .

.PHONY: build docker
.PHONY: fmt test clippy proto doc doc-deps check stats
.PHONY: ci ci-quick info security-audit
