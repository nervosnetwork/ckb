# Tx-Pool Validation and Machine Contracts

The literal objective is owned only by
[`architecture-contract.json#/optimization_goal`](../architecture-contract.json#/optimization_goal).
Validation can reject an unsupported claim; it cannot turn a green package,
test suite or finite quotient into `H_CLOSED`, `STATIC_ATTAIN`, product
acceptance or G0.

## Active control surface

| Path | Role | Authority boundary |
|---|---|---|
| [`architecture-contract.json`](../architecture-contract.json) | Stable objective vocabulary and declared hard constraints | Does not prove its own coverage or optimality |
| [`.independent-execution-plan`](../.independent-execution-plan) | Ordered phase, stop and retirement rules | Cannot substitute a completed later phase |
| [`scripts/check_security_manifest.py`](../scripts/check_security_manifest.py) | Read-only structural checker and disposable projection generator | Green means structure is self-consistent, not that an open scientific claim is closed |
| [`scripts/check_all.py`](../scripts/check_all.py) | Sole CI entry point for the structural checker | Must not grow a hidden list of legacy validators |
| [`security-regression-manifest.json`](../security-regression-manifest.json) | Generated current projection | Never hand-edit or cite as proof |
| [`.release-progress`](../.release-progress) | Generated human-readable projection | Never a second state authority |

The executable test universe is discovered from the current Cargo graph by
Nextest. There is no checked-in test-name inventory, allowlist of test modules,
mutation result authority or second executable tx-pool algorithm.

## Evidence layers

The layers are disjoint and cannot substitute for one another.

| Layer | Question | Completion evidence |
|---|---|---|
| Static safety | Do dependency, conflict, ownership, compatibility and bounded-resource invariants hold? | A named proof or executable production semantic counterexample |
| Finite empirical | What happens for one frozen binary/workload/environment matrix? | A complete legal matrix, otherwise the whole campaign is invalid |
| Literal G0 | Is one member globally static-optimal, empirically strongest and minimum-complexity over the open architecture class? | A member, a proved empty intersection, or explicitly OPEN |

Current finite normal-form results are scoped falsifiers only. The historical P3
instrument is frozen and contributes no G0 progress. Candidate/product outcomes
must not be observed before their owning static and execution gates authorize
them.

## Production property ownership

Properties that survived executable-model retirement are bound directly to
production types and production Plan/Apply observations. Small pure relations
under `authority/tests/claim_relations/` may normalize only the named
observation consumed by their test. They cannot construct, mutate or step a
tx-pool.

The retained property modules cover:

- clock reservation and overflow;
- controller/effect boundary ownership;
- contract and metrics observation;
- evidence currentness and missing-dependency policy;
- membership, ordering and eviction;
- effect publication and wake levels;
- released-input final-owner projection;
- continuous resource accounting;
- scheduler set/ring/wave transitions;
- settlement classification;
- stable-cut lifecycle traces.

A property must name the claim it checks. A filtered, ignored, undiscovered,
unstarted or model-only test proves nothing about the production universe.

## Commands

Run from the repository root. Formatting writes and checks are separate.

```bash
cargo fmt --all
make fmt
make check
make clippy
cargo nextest run -p ckb-tx-pool --features internal
make quick-test
make test
python3 -B tx-pool/scripts/check_all.py
git diff --check
```

Rules:

- Never invoke direct `cargo test` as tx-pool evidence. Repository-owned
  `make quick-test` or `make test` may run their declared doc-test step.
- Never invoke direct `cargo clippy`; use `make clippy`.
- Different absolute source roots must not share one `CARGO_TARGET_DIR`.
- Standalone Nextest `LEAK` is a known non-blocking false positive; test
  failures, skipped required tests and zero-match filters remain blockers.
- Network availability is not a project correctness premise.
- Full-workspace gates run at an owning boundary after focused affected tests.

## Formal falsifiers

The TLA modules under [`formal/`](../formal/) are historical
proposition-specific falsifiers. They are not a second production
specification, are not part of the default gate, and cannot close an
open-architecture claim. A future hard/static controller may bind an exact
module/config/runtime tuple to one named static proposition; until then the
modules are dormant historical inputs, not current evidence.

## Model-base retirement boundary

Retirement is complete only when all of the following are true:

1. no production test imports or mounts `mathematical_model`;
2. every retained invariant or counterexample has a production property or a
   claim-specific relation with a production endpoint;
3. model-only modules, scripts, certificates, generated inventories and their
   live documentation references are absent from the candidate tree;
4. the historical bytes remain recoverable outside the candidate tree;
5. focused and aggregate gates pass;
6. the next action is the named hard/static proposition, not another method or
   retirement generation.

Retirement proves only that semantic authority is no longer duplicated. It does
not close `H_CLOSED`, `STATIC_ATTAIN`, performance, complexity, product
acceptance or G0.

## Change discipline

- Reproduce the owning producer, consumer and observation before changing
  production behavior.
- Fix one owner relation; do not add finding-shaped flags, retries, scans,
  fallbacks or allowlists.
- Update production types, properties and public documentation together.
- Preserve unrelated user changes and never build the dirty shared main
  worktree.
- Before compaction or interruption, persist exact source identities, test
  results, blockers and the next named proposition in a cold-restorable
  checkpoint. Conversation history is not execution state.
