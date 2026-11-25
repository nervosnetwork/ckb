This directory contains integration tests that test CKB binary. It does not contain unit tests, which can be found in [/network/src/tests](/network/src/tests), etc.

## Running tests locally

Build the test node from the repository root with `cargo build --locked --bin ckb --release --features test,deadlock_detection`.
The `test` feature enables internal overrides such as the network verification time cap; production builds omit these configuration fields.

The following command assumes that CKB binary is built as `../target/release/ckb` and starting node on port 9000:

```bash
cargo run
```

Run specified specs:

```bash
cargo run -- --bin ../target/debug/ckb spec1 spec2
```

See all available options:

```bash
cargo run -- --help
```
