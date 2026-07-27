# Tx-Pool Validation Tools

Run every command from the repository root. The tools are tx-pool-owned and
live in [`tx-pool/scripts/`](../scripts/); they do not mutate production code.

| Command | Purpose | Writes by default |
|---|---|---|
| `python3 tx-pool/scripts/check_docs.py` | Validate documentation links, index coverage, script documentation, and retired path names. | No |
| `python3 tx-pool/scripts/check_review_guide.py` | Validate the behavior registry and the generated section of the review guide. | No |
| `python3 tx-pool/scripts/check_test_layout.py` | Enforce test isolation, reviewed module wiring, static panic restrictions, and test-only seams. | No |
| `python3 tx-pool/scripts/check_security_manifest.py` | Discover nextest tests and validate architecture/security evidence and inventories. | No |
| `python3 tx-pool/scripts/benchmark.py` | Run fingerprinted Criterion samples and controlled A/B comparisons. | Benchmark artifacts only when requested |

## Normal review gate

```bash
python3 tx-pool/scripts/check_docs.py
python3 tx-pool/scripts/check_review_guide.py
python3 tx-pool/scripts/check_test_layout.py
python3 tx-pool/scripts/check_security_manifest.py
cargo nextest run -p ckb-tx-pool --features internal
cargo clippy -p ckb-tx-pool --all-targets --features internal -- -D warnings
```

The first three checks are deterministic and form the dedicated lightweight
tx-pool documentation/contract CI flow. Security-manifest validation performs
nextest discovery and remains in the ordinary Rust CI, where nextest is
already installed.

## Intentional generated updates

After changing `review-behaviors.json`, regenerate only its owned guide region:

```bash
python3 tx-pool/scripts/check_review_guide.py --write
```

After deliberately adding, removing, renaming, or relocating tests, regenerate
the inventory and then re-run the read-only gates:

```bash
python3 tx-pool/scripts/check_security_manifest.py --update-inventory
python3 tx-pool/scripts/check_review_guide.py
python3 tx-pool/scripts/check_test_layout.py
python3 tx-pool/scripts/check_docs.py
```

For a built integration runner, validate the discovered process-test universe:

```bash
target/release/ckb-test --list-specs --bin target/release/ckb > /tmp/ckb-specs.txt
python3 tx-pool/scripts/check_security_manifest.py \
  --integration-only --integration-spec-list /tmp/ckb-specs.txt
```

`--release` additionally fails on explicit release blockers and belongs only
to final acceptance. `--write` and `--update-inventory` must never be used in
CI: CI detects drift; it does not rewrite the proposed change.

## Benchmark authorization

Benchmarking is a separate P7 acceptance phase. Do not infer performance from
unit-test duration and do not run the harness before explicit authorization.
When authorized, follow [the benchmark protocol](BENCHMARK.md); start with:

```bash
python3 tx-pool/scripts/benchmark.py --quick
```
