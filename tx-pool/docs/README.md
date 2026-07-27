# Tx-Pool Documentation

This directory is the human-readable entry point for tx-pool design and
review. Machine-readable contracts and inventories live one level above, next
to `Cargo.toml`; executable maintenance tools live in `../scripts/`.

| Document | Reviewer purpose |
|---|---|
| [Architecture](ARCHITECTURE.md) | Authoritative ownership, state, lock, effect, and failure model. |
| [Architecture audit](ARCHITECTURE_AUDIT.md) | Independent comparison with `develop` and proof of necessity. |
| [Pipeline](PIPELINE.md) | Operational data flow and implementation map. |
| [Review guide](REVIEW_GUIDE.md) | Test-driven behavior and hostile-case review registry. |
| [Implementation plan](IMPLEMENTATION_PLAN.md) | Checkpoints, acceptance gates, and remaining work. |
| [Security regression ledger](SECURITY_REGRESSION_LEDGER.md) | Historical findings and their invariant families. |
| [Validation tools](TOOLS.md) | Local and CI commands, generated artifacts, and update protocol. |
| [Benchmark protocol](BENCHMARK.md) | Controlled A/B methodology; run only when explicitly authorized. |

## Machine contracts

- [`architecture-contract.json`](../architecture-contract.json): frozen
  architecture vocabulary and invariants.
- [`review-behaviors.json`](../review-behaviors.json): source of truth for the
  generated behavior table in the review guide.
- [`test-layout-manifest.json`](../test-layout-manifest.json): allowed test
  roots, module wiring, and reviewed test-only seams.
- [`security-regression-manifest.json`](../security-regression-manifest.json):
  security evidence and release gates.
- [`integration-impact.json`](../integration-impact.json): managed integration
  test universe.
- [`test-inventory.txt`](../test-inventory.txt): generated Rust and integration
  test inventory.

Documents explain the contracts; JSON owns machine-consumed facts. Do not copy
the same list into another document or script.
