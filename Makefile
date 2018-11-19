test:
	RUSTFLAGS="--cfg ckb_test" cargo test --all -- --nocapture

build:
	cargo build --release

fmt:
	cargo fmt --all -- --check

clippy:
	cargo clippy --all -- -D warnings -D clone_on_ref_ptr -D unused_extern_crates -D enum_glob_use

ci: fmt clippy test
	git diff --exit-code Cargo.lock

ci-quick: test
	git diff --exit-code Cargo.lock

proto:
	protoc --rust_out network/protocol/src network/protocol/src/protocol.proto

info:
	date
	pwd
	env

cache-warm:
	cargo build --target-dir target-cache --tests -p rocksdb -p libp2p
	rsync -a -f"+ */" -f"- *" target-cache/ target/
	cd target-cache && find * -type f -exec ln -snf "$$(pwd)/{}" "../target/{}" \;

cache-clean:
	rm -f target-cache/.rustc_info.json
	@(( "$$(du -sm target-cache | cut -f1)" > 3000 )) && echo "Clean cache since it is larger then 3G!" && rm -rf target-cache && mkdir target-cache || true

.PHONY: build fmt test clippy ci ci-quick proto info
.PHONY: cache-warm cache-clean
