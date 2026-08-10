# ckb-tx-pool

## Canonical final goal

The sole machine authority for the program's final objective is
[`architecture-contract.json#/optimization_goal`](architecture-contract.json#/optimization_goal).
All tx-pool documents inherit its hard constraints and ordered `X0 -> X1 -> X2
-> X3` selection: feasible architectures, global static minima, noise-gated
empirical winners on the declared matrix, then minimum implementation/proof
complexity. Independent work is maximally parallel; coupled facts are ordered
only at the unique authority's minimum atomic commit cut. This README is an
index, not a second definition of that objective.

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
  architecture vocabulary, T1-T16 proof obligations, semantic refinement
  roots and residual-risk IDs.
- [`review-behaviors.json`](review-behaviors.json) owns the `TP-*` behavior and
  executable-evidence mapping.
- [`integration-impact.json`](integration-impact.json) owns the complete
  tx-pool-related process-spec universe.
- [`mutation-acceptance-lock.json`](mutation-acceptance-lock.json) is the
  generated row-level V1 mutation projection over the semantic obligations,
  exact tool/input hashes and complete library-test universe.
- [`mutation-result-lock.json`](mutation-result-lock.json) is the generated
  portable outcome projection over that exact candidate universe; it binds
  every unique candidate to one reconciled result and rejects inconsistent
  structural replays.
- [`security-regression-manifest.json`](security-regression-manifest.json)
  binds the other contracts, semantic mutation obligations and release
  blockers without copying generated counts or selectors.
- [`test-layout-manifest.json`](test-layout-manifest.json) allows only reviewed
  test roots, module wiring and test-only seams.
- [`test-inventory.txt`](test-inventory.txt) is the generated exact Rust and
  integration test inventory.

See [Validation and machine contracts](docs/VALIDATION.md) before editing any
of these files; it records which files are hand-maintained, which are generated
and which checks consume them.
