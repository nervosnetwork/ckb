VERBOSE := $(if ${CI},--verbose,)

test:
	cargo test ${VERBOSE} --all -- --nocapture

doc:
	cargo doc --all --no-deps

doc-deps:
	cargo doc --all

check:
	cargo check ${VERBOSE} --all

build:
	cargo build ${VERBOSE} --release

prod:
	RUSTFLAGS="--cfg disable_faketime" cargo build ${VERBOSE} --release

prod-test:
	RUSTFLAGS="--cfg disable_faketime" RUSTDOCFLAGS="--cfg disable_faketime" cargo test ${VERBOSE} --all -- --nocapture

fmt:
	cargo fmt ${VERBOSE} --all -- --check

clippy:
	cargo clippy ${VERBOSE} --all -- -D warnings -D clippy::clone_on_ref_ptr -D clippy::enum_glob_use

ci: fmt clippy test
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

docker:
	docker build -f docker/hub/Dockerfile -t nervos/ckb:latest .

.PHONY: build prod prod-test docker
.PHONY: fmt test clippy proto doc doc-deps check stats
.PHONY: ci info security-audit
