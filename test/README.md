This directory contains integration tests that test CKB binary. It does not contain unit tests, which can be found in [/network/src/tests](/network/src/tests), etc.

## Running tests locally
Before tests can be run locally, CKB binary must be built. See the [build from source & testing](/README.md#build-from-source--testing) for help.

The following command assumes that CKB binary is built as `../target/release/ckb` and starting node on port 9000:

```bash
cargo run
```

Run single spec:

```bash
cargo run ../target/debug/ckb 9000 spec_name
```
