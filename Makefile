FLATC   := flatc
CFBC    := cfbc
VERBOSE := $(if ${CI},--verbose,)

test:
	cargo test ${VERBOSE} --all -- --nocapture

doc:
	cargo doc --all --no-deps

doc-deps:
	cargo doc --all

check:
	cargo check ${VERBOSE} --all
	cd test && cargo check ${VERBOSE} --all

build:
	cargo build ${VERBOSE} --release

prod:
	RUSTFLAGS="--cfg disable_faketime" cargo build ${VERBOSE} --release

prod-test:
	RUSTFLAGS="--cfg disable_faketime" RUSTDOCFLAGS="--cfg disable_faketime" cargo test ${VERBOSE} --all -- --nocapture

fmt:
	cargo fmt ${VERBOSE} --all -- --check
	cd test && cargo fmt ${VERBOSE} --all -- --check

clippy:
	cargo clippy ${VERBOSE} --all --all-targets --all-features -- -D warnings -D clippy::clone_on_ref_ptr -D clippy::enum_glob_use -D clippy::fallible_impl_from
	cd test && cargo clippy ${VERBOSE} --all --all-targets --all-features -- -D warnings -D clippy::clone_on_ref_ptr -D clippy::enum_glob_use -D clippy::fallible_impl_from


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

GEN_FILES := protocol/src/protocol_generated.rs protocol/src/protocol_generated_verifier.rs
gen: ${GEN_FILES}
gen-clean:
	rm -f ${GEN_FILES}

%_generated.rs: %.fbs
	$(FLATC) -r -o $(shell dirname $@) $<

%_generated_verifier.rs: %.fbs
	$(FLATC) -b --schema -o $(shell dirname $@) $<
	$(CFBC) -o $(shell dirname $@) $*.bfbs
	rm -f $*.bfbs $*_builder.rs

.PHONY: build prod prod-test docker gen gen-clean
.PHONY: fmt test clippy doc doc-deps check stats
.PHONY: ci info security-audit
