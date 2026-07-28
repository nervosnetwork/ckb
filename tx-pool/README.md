# ckb-tx-pool

This crate is a component of [ckb](https://github.com/nervosnetwork/ckb).

CKB Tx-pool stores transactions for CKB's two-step transaction confirmation
mechanism.

## Design and review

| Document | Purpose |
|---|---|
| [Architecture](docs/ARCHITECTURE.md) | Normative ownership, state, Plan/Apply, locking, effects, failure model, residual risks and release conditions. |
| [Performance design and evidence](docs/PERFORMANCE.md) | Optimization constraints, retained/rejected designs, profiling evidence and fixed acceptance plan. |
| [Test-driven review guide](docs/REVIEW_GUIDE.md) | Stable behaviors, hostile cases, source navigation and executable evidence. |
| [Validation and machine contracts](docs/VALIDATION.md) | JSON/TXT responsibilities, maintenance commands, generated artifacts and CI rules. |
| [Benchmark protocol](docs/BENCHMARK.md) | Controlled, fingerprinted performance A/B methodology. |

The machine-readable contracts consumed by validation and CI live beside this
README:

- [`architecture-contract.json`](architecture-contract.json) freezes the
  architecture vocabulary, T1–T13 proof obligations and residual-risk IDs.
- [`review-behaviors.json`](review-behaviors.json) owns the `TP-*` behavior and
  executable-evidence mapping.
- [`integration-impact.json`](integration-impact.json) owns the complete
  tx-pool-related process-spec universe.
- [`security-regression-manifest.json`](security-regression-manifest.json)
  binds the other contracts, inventory counts and release blockers.
- [`test-layout-manifest.json`](test-layout-manifest.json) allows only reviewed
  test roots, module wiring and test-only seams.
- [`test-inventory.txt`](test-inventory.txt) is the generated exact Rust and
  integration test inventory.

See [Validation and machine contracts](docs/VALIDATION.md) before editing any
of these files; it records which files are hand-maintained, which are generated
and which checks consume them.
