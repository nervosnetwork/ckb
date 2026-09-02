# Tx-Pool Validation and Machine Contracts

The objective and hard constraints are declared by
[`architecture-contract.json`](../architecture-contract.json). Live phase,
source identity, blockers and next action are owned only by the manifest-bound
[`control/txpool-v8/`](../control/txpool-v8/) state.
Validation can reject an unsupported claim; it cannot turn a green package,
test suite or finite quotient into `H_CLOSED`, `STATIC_ATTAIN`, product
acceptance or G0.

## Active control surface

| Path | Role | Authority boundary |
|---|---|---|
| [`architecture-contract.json`](../architecture-contract.json) | Stable objective vocabulary, hard constraints, phase and Acceptance contract | Not live execution state; does not prove its own coverage or optimality |
| [`control/txpool-v8/`](../control/txpool-v8/) | Repository-owned project state: live identity/phase/next action plus active audit and on-demand evidence | Open findings remain open until Primary reproduction and repaired-identity audit |
| [`scripts/check_security_manifest.py`](../scripts/check_security_manifest.py) | Read-only structural checker and disposable projection generator | Green means structure is self-consistent, not that an open scientific claim is closed |
| [`scripts/check_all.py`](../scripts/check_all.py) | Sole CI entry point for the structural checker | Must not grow a hidden list of legacy validators |
| [`security-regression-manifest.json`](../security-regression-manifest.json) | Generated disposable projection of the stable contract and project state | Never hand-edit or cite as proof/current state |
| [`.release-progress`](../.release-progress) | Generated human-readable projection | Never a second state authority |

The executable test universe is discovered from the current Cargo graph by
Nextest. There is no checked-in test-name inventory, allowlist of test modules,
mutation result authority or second executable tx-pool algorithm.

## Evidence layers

The layers are disjoint and cannot substitute for one another.

| Layer | Question | Deciding evidence |
|---|---|---|
| Static safety | Do dependency, conflict, ownership, compatibility and bounded-resource invariants hold? | A named production-bound proof can uphold the scoped invariant; an executable semantic counterexample keeps it open and selects a repair root |
| Finite empirical | What happens for one frozen binary/workload/environment matrix? | A complete legal matrix, otherwise the whole campaign is invalid |
| Open-class research G0 | Is one member globally static-optimal, empirically strongest and minimum-complexity over the frozen production-realizable architecture class? | A member, a proved empty intersection, or explicitly `OPEN_FROZEN`; finite-candidate evidence cannot close it |
| Canonical finite-candidate delivery | Does one result-before-frozen candidate pass every hard/terminal gate and dominate or win the predeclared finite comparison without an unresolved Pareto choice? | Exact candidate generation, repaired-identity gates, valid comparative evidence, complexity/security/reviewer Acceptance, or explicitly OPEN |

Current finite normal-form results are scoped falsifiers only. The historical
P3 instrument is frozen and contributes no current delivery evidence. The
open-class theorem remains research `OPEN_FROZEN`; it no longer blocks the
separately owned finite-candidate delivery decision. Candidate/product outcomes
must not be observed before terminal correctness and a new result-before-frozen
candidate contract authorize them.

## Production property ownership

Properties that survived executable-model retirement are bound directly to
production types and production Plan/Apply observations. Small pure relations
under `authority/tests/claim_relations/` may normalize only the named
observation consumed by their test. They cannot construct, mutate or step a
tx-pool.

The retained property modules cover:

- non-malformed retained ingress uses one coherent `Owner | EffectOrNoop`
  head classification; owner/effect mixed batches return exact prefixes,
  disjoint cuts overlap, and a classification flip becomes scoped operational
  contention rather than an outer-write fallback or generation fault;
- malformed Remote ingress installs one hidden routed peer fence before taking
  its exact `PreAccepted` cohort snapshot; same-peer owner changes stale,
  disjoint peer revocations overlap inside real final cuts, cross-peer equal raw
  hashes remain independent, effect-capacity failure and dropped plans restore
  every hidden capability, capacity replacement preserves victim owners, active
  fences survive generation replacement, and a fence-only fixture attains the
  exact `2 * PEER_BAN_FENCE_CAPACITY` extra-row bound without claiming that
  owner-bearing peer rows share the slot-bank bound;
- fresh-generation replacement keeps the live routed shard layout and active
  fence identity across repeated clears, swaps exactly the generation-owned
  payload into a private retirement carrier, and leaves no recoverable fence
  allocation or owner-population scan in the terminal;
- the fixed peer-ban slot bank preserves reservation order under reverse
  completion, restores the exact oldest after rollback, and refuses to select a
  victim while any full-bank slot hides an in-flight prior order;
- staged rejection/duplicate/release effects are invisible before activation,
  rollback releases exact capacity, and activation derives its wake from the
  same `EffectLog` cut;

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
make test
python3 -B tx-pool/scripts/check_all.py
git diff --check
```

Rules:

- Never invoke direct `cargo test` as tx-pool evidence. Use `make test`
  directly when the full aggregate universe is required; do not precede it
  with `make quick-test`.
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

The synchronized `73f58c604` implementation contains no second executable
tx-pool model or semantic authority. This section is a permanent regression
boundary, not an open migration task.

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
