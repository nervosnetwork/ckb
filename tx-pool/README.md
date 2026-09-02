# ckb-tx-pool

## Objective and current state

The stable objective and hard-constraint contract is
[`architecture-contract.json#/final_goal`](architecture-contract.json#/final_goal).
The manifest-bound
[`control/txpool-v8/STATE.json`](control/txpool-v8/STATE.json)
is the only live execution state. `DOCUMENT_AUTHORITY.json` classifies every
other design, review, validation, performance and historical artifact. This
README is an index, not another objective or status ledger.

The original open-architecture goal is retained as research. The canonical
delivery target is the next result-before-frozen candidate generation under
the decisions in `CKB_AUTHORITY_INPUT_LEDGER.md`. The synchronized true-shard
route migration is undergoing terminal correctness/root-repair audit and is
not yet a performance, security or Acceptance winner.

This crate is a component of [ckb](https://github.com/nervosnetwork/ckb).

CKB Tx-pool stores transactions for CKB's two-step transaction confirmation
mechanism.

## Design and review

| Document | Purpose |
|---|---|
| [Engineering operating manual](control/txpool-v8/OPERATING_SYSTEM.md) | Human-readable control plane, method, audit discipline, command contract and cold-continuity workflow; manifest-bound machine files remain literal authority. |
| [Architecture](docs/ARCHITECTURE.md) | Intended ownership, state, Plan/Apply, locking, effects and failure model; frozen source and project state win on conflict. |
| [Performance design and evidence](docs/PERFORMANCE.md) | Optimization constraints, retained/rejected designs, profiling evidence and fixed acceptance plan. |
| [Test-driven review guide](docs/REVIEW_GUIDE.md) | Stable behaviors, hostile cases, source navigation and executable evidence. |
| [Validation and machine contracts](docs/VALIDATION.md) | JSON/TXT responsibilities, maintenance commands, generated artifacts and CI rules. |
| [Benchmark protocol](docs/BENCHMARK.md) | Controlled, fingerprinted performance A/B methodology. |

The active machine control surface is deliberately small:

- [`control/txpool-v8/STATE.json`](control/txpool-v8/STATE.json) alone owns
  live identity, phase, root, claim status and the next atomic action;
  `AUDIT_PLAN.json` owns only the current frozen audit round.
- [`architecture-contract.json`](architecture-contract.json) owns stable
  objective vocabulary, hard constraints, phase and Acceptance rules; it is
  not another live status pointer.
- `STATE.json.next_atomic_action` alone owns the exact next work. The audit
  plan must reference the same cut; the contract owns only phase order.
- [`security-regression-manifest.json`](security-regression-manifest.json) and
  [`.release-progress`](.release-progress) are disposable projections produced
  by `scripts/check_security_manifest.py`; neither is proof.
- `scripts/check_all.py` is the only CI entry point and delegates to that one
  checker.

Production properties live beside the production authority tests and are
discovered by Nextest. There is no static test-name inventory and no second
executable tx-pool model. See [Validation](docs/VALIDATION.md) before changing
the control surface or claim boundaries.
