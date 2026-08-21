# ckb-tx-pool

## Canonical final goal

The sole machine authority for the program's final objective is
[`architecture-contract.json#/optimization_goal`](architecture-contract.json#/optimization_goal).
All tx-pool documents inherit its hard constraints and ordered `feasible_set -> static_minima -> empirical_survivors
-> complexity_minima` selection: feasible architectures, global static minima, noise-gated
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

The active machine control surface is deliberately small:

- [`architecture-contract.json`](architecture-contract.json) owns vocabulary
  and the literal objective; finite historical normal forms do not prove its
  open-architecture quantifiers.
- [`.independent-execution-plan`](.independent-execution-plan) owns the ordered
  phase and retirement gates.
- [`security-regression-manifest.json`](security-regression-manifest.json) and
  [`.release-progress`](.release-progress) are disposable projections produced
  by `scripts/check_security_manifest.py`; neither is proof.
- `scripts/check_all.py` is the only CI entry point and delegates to that one
  checker.

Production properties live beside the production authority tests and are
discovered by Nextest. There is no static test-name inventory and no second
executable tx-pool model. See [Validation](docs/VALIDATION.md) before changing
the control surface or claim boundaries.
