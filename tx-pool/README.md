# ckb-tx-pool

This crate implements CKB transaction admission, dependency handling,
verification, pool membership, relay effects and block-template inputs.

The delivery objective and hard constraints live in
[`architecture-contract.json`](architecture-contract.json). The only live
engineering cut is [`control/txpool-v8/STATE.json`](control/txpool-v8/STATE.json);
run [`scripts/check_all.py`](scripts/check_all.py) to validate it. Stable
authority decisions are in
[`control/txpool-v8/CKB_AUTHORITY_INPUT_LEDGER.md`](control/txpool-v8/CKB_AUTHORITY_INPUT_LEDGER.md),
and unresolved findings remain in
[`control/txpool-v8/FINDINGS_LEDGER.json`](control/txpool-v8/FINDINGS_LEDGER.json).

Reviewer entry points:

- [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md): incumbent state and atomicity boundaries.
- [`docs/REVIEW_GUIDE.md`](docs/REVIEW_GUIDE.md): necessity, concurrency and maintenance review.
- [`docs/PERFORMANCE.md`](docs/PERFORMANCE.md): causal performance evidence and guardrails.
- [`docs/BENCHMARK.md`](docs/BENCHMARK.md): reproducible comparison protocol.
- [`PROFILING.md`](PROFILING.md): maintained capture and analysis commands.

Tests are discovered from Cargo/Nextest rather than a checked-in test-name
inventory. Benchmark and profiling tools are maintained source, not disposable
experiments. Partner-R, agent reports and generated views are evidence inputs,
never execution gates or semantic authorities.
