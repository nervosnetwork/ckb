# Tx-Pool Validation and Machine Contracts

This document is the maintenance entry point for the architecture and review
contracts. Run commands from the repository root. Validators are read-only by
default; CI detects drift and never rewrites a proposed change.

The normative design is [`ARCHITECTURE.md`](ARCHITECTURE.md). The behavior and
test-facing contract is [`REVIEW_GUIDE.md`](REVIEW_GUIDE.md).

## Machine-readable files

These files live in `tx-pool/` so local tools and CI can consume them without
parsing prose. JSON files are intentionally hand-maintained unless the table
says otherwise.

| File | Authority and purpose | Consumer | Update rule |
|---|---|---|---|
| [`architecture-contract.json`](../architecture-contract.json) | Canonical names for the two authorities, six states, identity domains, Plan outcomes, lock order, T1–T13 obligations and current residual-risk IDs. It prevents prose and validators from silently describing different models. | `check_security_manifest.py` | Edit with an architecture change; update `ARCHITECTURE.md`, behavior evidence and tests in the same PR. |
| [`review-behaviors.json`](../review-behaviors.json) | Stable `TP-*` behaviors, hostile cases, change surfaces, minimum commands, T1–T13 mappings and focused unit/integration evidence. It is the source for the generated region in `REVIEW_GUIDE.md`. | `check_review_guide.py`, `check_security_manifest.py`, `check_test_layout.py` | Hand-edit for a behavior/evidence change, then run `check_review_guide.py --write`. Do not edit the generated Markdown region. |
| [`integration-impact.json`](../integration-impact.json) | Curated complete set of registered process specs whose production paths cross tx-pool ingress, verification, pool mutation, relay, mining/template or transaction-bearing reorg boundaries. | `check_review_guide.py`, `check_security_manifest.py` | Hand-edit when a relevant spec is added, removed, renamed or its boundary changes. Integration CI checks it against `ckb-test --list-specs`. |
| [`security-regression-manifest.json`](../security-regression-manifest.json) | Assembly manifest binding the crate/features, architecture contract, behavior registry, integration universe, frozen inventory counts and explicit release blockers. It does not duplicate individual test evidence. | `check_security_manifest.py` | Hand-edit only when one of those top-level contracts, counts or release conditions changes. |
| [`test-layout-manifest.json`](../test-layout-manifest.json) | Allowlist for test roots, production-module wiring, `cfg(test)` counts and reviewed test-only seams. It prevents tests from drifting back into production files or changing production behavior invisibly. | `check_test_layout.py` | Hand-edit only with an intentional test-layout or reviewed seam change. Never weaken it merely to accept drift. |
| [`test-inventory.txt`](../test-inventory.txt) | Exact sorted snapshot of discovered internal-feature Rust tests and the managed integration spec names. This is generated evidence, not a hand-written selection list. | `check_security_manifest.py` | Regenerate with `--update-inventory` after an intentional test/integration inventory change, then review the diff. |

The ownership direction is one-way:

```text
ARCHITECTURE.md
      │ vocabulary and obligations
      ▼
architecture-contract.json
      │
      ├── review-behaviors.json ──generates──► REVIEW_GUIDE.md
      ├── integration-impact.json
      ├── test-layout-manifest.json
      └── security-regression-manifest.json ──validates──► test-inventory.txt
```

Machine files own facts that need exact comparison; Markdown explains why
those facts exist. Do not copy a machine-owned list into another script or
hand-maintained table.

## Tools

| Command | Purpose | Writes by default |
|---|---|---|
| `python3 tx-pool/scripts/check_docs.py` | Validate links, root index coverage, script/contract documentation and retired names. | No |
| `python3 tx-pool/scripts/check_review_guide.py` | Validate the behavior registry and generated review-guide region. | No |
| `python3 tx-pool/scripts/check_test_layout.py` | Enforce test isolation, module wiring, static panic restrictions and reviewed test-only seams. | No |
| `python3 tx-pool/scripts/check_security_manifest.py` | Discover nextest tests and validate the architecture, behavior, integration and inventory contracts. | No |
| `python3 tx-pool/scripts/benchmark.py` | Produce fingerprinted Criterion records and controlled A/B comparisons. | Only requested benchmark artifacts |
| `python3 tx-pool/scripts/profile.py capture ...` | Build or reuse one hashed profiling binary, capture a windowed Samply profile and emit a strict manifest plus deterministic summary. Artifacts must be outside the source tree. | Only the requested external artifact prefix |
| `python3 tx-pool/scripts/profile.py analyze --manifest ...` | Revalidate recorded artifact hashes and regenerate the window-cropped symbol summary without executing CKB code. | The summary path owned by the manifest |

## Normal review gate

```bash
python3 tx-pool/scripts/check_docs.py
python3 tx-pool/scripts/check_review_guide.py
python3 tx-pool/scripts/check_test_layout.py
python3 tx-pool/scripts/check_security_manifest.py
cargo nextest run -p ckb-tx-pool --features internal
cargo clippy -p ckb-tx-pool --all-targets --features internal -- -D warnings
```

The dedicated lightweight workflow
`.github/workflows/ci_tx_pool_review.yaml` compiles the scripts and runs the
deterministic documentation, behavior and layout checks. Security-manifest
validation stays with Rust CI because it performs nextest discovery.

## Intentional updates

After changing `review-behaviors.json`, regenerate only its owned Markdown
region:

```bash
python3 tx-pool/scripts/check_review_guide.py --write
```

After deliberately adding, removing or renaming tests, regenerate the exact
inventory and then run every read-only validator:

```bash
python3 tx-pool/scripts/check_security_manifest.py --update-inventory
python3 tx-pool/scripts/check_review_guide.py
python3 tx-pool/scripts/check_test_layout.py
python3 tx-pool/scripts/check_docs.py
```

For a built integration runner, validate that the curated impact set still
exists in the executable process-test universe:

```bash
target/release/ckb-test --list-specs --bin target/release/ckb > /tmp/ckb-specs.txt
python3 tx-pool/scripts/check_security_manifest.py \
  --integration-only --integration-spec-list /tmp/ckb-specs.txt
```

`--release` additionally fails while
`security-regression-manifest.json` contains a release blocker. `--write` and
`--update-inventory` are maintainer commands and must not run in CI.

## Performance evidence

Correctness tests and their duration are not performance evidence. Follow
[`BENCHMARK.md`](BENCHMARK.md) for clean, repeated, fingerprint-matched A/B
records. A quick run is diagnostic; only the specified controlled comparison
can close the performance release condition. Follow
[`PERFORMANCE.md`](PERFORMANCE.md) for the profiling scenario contract, exact
Samply/Tokio-console/cargo-instruments commands, artifact schemas and the rule
that sampled stacks select candidates but never prove a throughput win.
