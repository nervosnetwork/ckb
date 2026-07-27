# Tx-Pool Root-Cause Refactor — Live Execution Plan

Design authority: [`ARCHITECTURE.md`](ARCHITECTURE.md)
Independent audit: [`ARCHITECTURE_AUDIT.md`](ARCHITECTURE_AUDIT.md)
Review evidence: [`REVIEW_GUIDE.md`](REVIEW_GUIDE.md)

Status date: 2026-07-27
Current stable code checkpoint: `dd95e1f99`
Preceding architecture checkpoint: `9e559a482`
Current phase: P6.6 review topology accepted; P7 controlled performance
acceptance remains frozen pending explicit authorization

This file is the live execution ledger. It is updated at every checkpoint so a
compaction or restart can resume from the actual code rather than an obsolete
design stage. The architecture, audit, pipeline, review registry, test-layout
and security documents were migrated to the final six-state/Rust-native model
in P5; any historical mechanism named by the security ledger is evidence, not
current implementation authority.

## 1. Frozen direction

The refactor is allowed to retain only concepts needed to prove a develop bug
or a stated security/performance boundary.

1. There is one logical transaction-owner sum (`Absent | PrePool | Accepted`),
   represented by two mutually exclusive physical partitions: accepted
   `TxPool` and pre-accepted `PrePoolKernel`. The split preserves accepted-read
   concurrency; one typed cross-partition admission protocol proves the sum.
2. A pre-accepted transaction occupies exactly one of six locations:
   `ResolveQueued`, `ResolveLeased`, `Wait`, `VerifyQueued`, `VerifyLeased`, or
   `Ready`. Missing/conflict are reasons within `Wait`; recovery is a trusted
   source, not a seventh location.
3. Local submission remains the direct synchronous path by design. Remote,
   Proposal and reorg Recovery use the retained pipeline only where their
   scheduling/liveness requirements need it.
4. Ordinary accepted-pool mutation is immutable Plan followed by total Apply.
   No nested undo, rollback journal, speculative victim owner, or post-removal
   policy decision may return.
5. Derived indexes and accounting are projections of primary ownership. They
   may be rebuilt in tests, but production does not repair contradictions and
   continue.
6. Effect publication is sequenced and bounded. Remote traffic cannot consume
   trusted or chain-critical headroom. Ordinary admission uses an immutable
   exact batch and never substitutes a post-Apply reset; only chain/admin
   authority may converge saturated detail through `GenerationReset`.
7. Performance is a hard acceptance criterion. Hot single-entry transitions
   keep stack-sized plans; the heap-backed cohort planner is reserved for
   bounded multi-entry transitions. No benchmark runs before P7.
8. Production line counts exclude tests and benchmarks. There is no arbitrary
   line target: growth must be justified by removed develop failure modes and
   measured performance, while every safe deletion remains required.
9. Rust safety order is fixed: make an invalid state unconstructable first,
   otherwise return a typed result before mutation, and isolate only genuinely
   foreign code. `panic!` plus `catch_unwind` is forbidden as transaction,
   worker, rollback, retry or authority-control flow; it cannot substitute for
   ownership, typestate or an explicit result.

## 2. Rust-native failure contract

The final develop comparison must audit this contract explicitly.

| Outcome | Encoding | Service effect |
|---|---|---|
| malformed/policy rejection | typed `Reject`/`PrePoolError` | reject or ban according to policy; service remains live |
| bounded capacity/backpressure | typed `Result` | no mutation or bounded optional-history degradation |
| stale asynchronous lease/race | typed stale/duplicate outcome | discard, retry the level, or preserve the exact current owner |
| shutdown/channel/config/resource outcome | typed at its external boundary | controlled shutdown or startup failure |
| resolver/verifier failure | typed job result before settlement | terminalize/quarantine the owned job; no state repair protocol |
| foreign callback/endpoint failure or hang | thread/task/channel isolation plus typed timeout/channel outcome | no authority unwind; committed state remains final |
| primary/index/accounting contradiction | typed system fault detected before mutation | controlled stop, no persistence; never an RPC/peer outcome |

The target is not an Erlang-style restart framework and does not claim survival
after OOM, abort or memory corruption. It does require panic-free transaction
and authority code: legal hostile input must never reach a process unwind, and
an internally inconsistent projection must be detected before mutation as a
distinct system fault rather than encoded by `assert`/`expect` or converted to
a transaction rejection.

## 3. Checkpoints

| Checkpoint | Commit | Meaning |
|---|---|---|
| C0 | `35cabc9b7` | pre-redesign coordinator baseline |
| C1 | `02e648255` | audited fixes and rollback/A-B base |
| C2 | `8596c6c5d` | formal target and evidence base |
| C3 | `1d9e0cf5b` | single-owner `PrePoolKernel` cutover |
| C4 | `7219778be` | accepted `PoolMutationPlan`/total Apply cutover |
| C5 | `74a5049bd` | statically partitioned effect journal |
| C6 | `d9aac44e4` | retained reorg recovery and chain convergence |
| C7 | `e3ab95375` | unified generation-reset disposal |
| C8 | `473b4e927` | worker supervision and persistence eligibility |
| C9 | `693413cf6` | simplified recovery/failure boundaries |
| C10 | `1e0d0098d` | total bounded cohort transitions |
| C11 | `77dcbb0c1` | relay saturation coalesces to durable reconciliation |
| C12 | `015d88be2` | Rust-native invariants and level-triggered derived recovery |
| C13 | `6d0577ad4` | move-only authority Apply and redundant-envelope removal |
| C14 | `288031ebc` | six-state production contract and evidence acceptance |
| C15 | `64ecdd0eb` | complete correctness, liveness and managed-integration acceptance |
| C16 | `eb26bd272` | evidence-only correctness checkpoint before admission convergence review |
| C17 | `9e559a482` | P6.5 exact admission, typed authority, static Rust proof and correctness acceptance |
| C18 | `dd95e1f99` | post-regression correctness freeze before mechanical review-layout cleanup |

No checkpoint before P7 is a performance verdict.

## 4. Completed work

### P0 — Contract and reference evidence

- Frozen ownership, state, budget, identity, effect and lock-order contracts.
- Added an independent recomputing reference model and generated transition
  tests.
- Established review/security manifests and test inventory. They must be
  regenerated in P5 because their prose still describes superseded designs.

### P1 — PrePoolKernel cutover

- Removed the legacy multi-queue inferred-location ownership model.
- Installed one concrete primary entry map, exact versioned leases, derived
  queues/indexes, per-peer/global residency and active-work bounds.
- Collapsed Missing and historical Conflict into `Wait` and preserved Local as
  direct submission.
- Removed `RaceLost`, `Invalidated`, `Committing`, dual incarnation/revision
  ownership and standalone conflict payload ownership.

### P2 — Accepted-pool Plan/Apply

- Replaced RBF/capacity mutation and nested undo with one read-only
  `PoolMutationPlan` and total Apply.
- Bounded the multi-input conflict union by the full input×candidate product.
- Separated causal producer links from conditional dep-reader ordering.
- Added role-aware post-RBF liveness, sparse virtual eviction and deterministic
  selected-set cycle handling.
- Audited tx verification cache access: current tx-pool paths construct
  `TxVerificationCacheKey::from_transaction`, hence use witness hash.

### P3 — Stable bounded effects and chain recovery

- Replaced carried effect reservations with static Remote/Trusted/Critical
  regions and one global ordered publisher.
- Made callback and external endpoint isolation explicit and bounded.
- Preserved a constant-size generation reset across FIFO saturation.
- Fixed relayer-full loss: individual terminal edges coalesce to a reset that
  remains authoritative until the receiver can accept it.
- Removed accounting recomputation-after-drift; poisoned journal state now
  fails fast instead of continuing after an invariant panic.
- Reorg authoritative mutation runs once; only compact derived template work
  remains retryable. Recovery is a trusted source in the six-state kernel.
- Preserved block assembler semantics: Reset/update_full share one ordered full
  publication boundary while construction remains concurrent, update_full has
  priority and derives from the bounded candidate-uncle authority, and
  optimistic uncle/proposal/transaction deltas remain concurrent, versioned
  and level-triggered.

## 5. P4 — Complete: failure semantics and authority acceptance

### Completed

- Removed `PrePoolError::Repair` and the generic Defect error class.
- Historical note, superseded by P6.5: P4 converted primary/index/accounting
  contradictions to assertions inside the kernel authority. The later static
  panic-surface audit showed this still relied on runtime proof and is not an
  acceptable final Rust encoding; P6.5 must remove it without weakening typed
  rejection, capacity, duplicate and stale outcomes.
- Made source attribution and `u128` clocks total under their construction
  invariants.
- Made dependency-availability notification a total transition rather than a
  fictitious fallible API.
- Found a real legal race during the full nextest gate: a selected Ready ticket
  could retain the same version while a later higher-ranked Ready owner became
  the scheduling head. The old validator returned stale and the caller
  panicked. The root correction validates the ticket's exact owner/version/rank
  only; the later owner remains Ready for the next serialized commit. A
  deterministic regression now covers the interleaving.

### Remaining P4 work

- [x] Re-run clippy and all `ckb-tx-pool` nextest tests after the ticket fix
      (232/232 before the retry-protocol deletion).
- [x] Audit every retry loop. The retained reorg phase-two/backoff protocol was
      removed: an assembler failure now leaves the existing reset and
      uncle/proposal/transaction generations dirty, while later chain deltas keep
      flowing. Remaining retries are capacity-notified or level-triggered
      external/derived work; no authoritative mutation is replayed.
- [x] Complete the call-graph proof. A capacity hint takes and releases the
      journal lock first; nested mutation then follows
      `TxPool → PrePoolKernel → effect journal`, with no reverse await or I/O
      under a state lock. Kernel-only worker transitions are the sole shorter
      form.
- [x] Verify every cross-authority pool plan settles kernel ownership before
      accepted Apply. Expected policy/capacity decisions finish in Plan;
      kernel operations after Plan are continuous-reservation/identity
      obligations and fail-fast only on a structural contradiction.
- [x] Classify residual startup/config fail-fast separately: minimum pre-pool
      residency, effect-region construction and dispatcher permit conversion
      are administrator/startup validation, not remotely selected transaction
      outcomes. Final documentation must keep them visible as residual risk.
- [x] Re-run full clippy/nextest after deleting the retained reorg retry
      protocol: clippy is clean and nextest passes 227/227.
- [x] Commit the passing P4 checkpoint and freeze the state/failure model as
      `015d88be2`.

### P4 exit gate

- legal transaction, concurrency, reorg, capacity and endpoint outcomes cannot
  trigger service fail-stop;
- internal contradictions cannot be converted to an RPC rejection or repaired
  into continued execution;
- no generic retry replays an authoritative mutation;
- clippy and full library nextest pass;
- the phase adds no owner, location, lock or recovery mechanism.

## 6. P5 — Global simplification and documentation acceptance

### Completed code convergence

- Cohort Apply now moves old/new primary entries and records only prior
  existence; immutable planning no longer clones every old primary into the
  plan. Single-entry transitions remain stack-sized and do not pass through
  the cohort planner.
- Resolve/verify, wait/promotion, commit metadata and trusted proposal
  promotion no longer make redundant full-entry clones. The remaining cohort
  clones create independently mutated proposed primaries and are required by
  read-only Plan semantics.
- Removed `KernelDisposal` and `AuthoritativeDisposal`: a sealed retired
  generation is the disposal capability and is destroyed only after releasing
  the accepted-pool guard.
- Collapsed `AppliedRemovalBatch`, `SubmitSideEffects` and
  `CoordinatedSubmitOutcome` into one `SubmitEntryOutcome` crossing the lock
  boundary. This removes transport envelopes without weakening effect order.
- Reorg recovery now compiles one parent-first accepted/detached recovery plan
  after the chain mutation and applies that exact plan; it no longer traverses
  and clones the accepted recovery set twice.
- Unified the zero/nonzero block-assembler loops while preserving Reset as a
  hard full-template barrier and ordinary uncle/proposal/transaction updates
  as concurrent level-triggered optimistic deltas. Update queues move rather
  than clone.
- Removed production `PipelineState::chunk_rx`; worker receivers belong to the
  worker runtime and direct white-box observation belongs to the test harness.
- Candidate-uncle tests now exercise production limits (128 candidates/10
  selected) instead of changing scheduling behavior under `cfg(test)`.
- Deleted stale multi-queue/orphan terminology at production boundaries.

### Completed architecture/evidence acceptance

- Recounted physical Rust lines with tests and benchmark excluded from
  production: C13 has 18,532 production lines versus C1's 24,236 (−5,704) and
  develop's 7,297 (+11,235). Tests are 13,602 lines and benchmark is 1,463;
  neither is used to hide production growth.
- Re-audited all six locations, two authorities, lock hold/await order,
  allocations, retained snapshots, graph/worker/effect bounds and hostile
  input paths. No new owner, state, rollback, repair or generic retry was
  accepted.
- Rewrote `ARCHITECTURE.md`, `ARCHITECTURE_AUDIT.md`, `pipeline.md`, the review
  guide/registry, machine contract, security ledger/manifests and test
  inventory against the final six-state/Rust-native model.
- Test isolation now validates 23 external module wires, 28 explicit
  `cfg(test)` sites and only two named production observation seams. Candidate
  uncle tests use production limits.
- The regenerated evidence maps 16 behaviors to current Rust anchors and 16
  focused integrations; the complete managed process universe is now 150
  after adding the Local test-RPC direct-path regression.
- The internal-feature gate found a real benchmark-harness authority bug: its
  direct service dropped the only worker command sender and stalled a two-tx
  dependency case at 1/2 for 120 seconds. The handle now owns command authority
  until cancellation; the regression passes in under two seconds and is in the
  review registry/inventory.
- All three document validators pass. All-target internal-feature clippy is
  warning-free and `cargo nextest run -p ckb-tx-pool --features internal`
  passes 228/228 in 21.9 seconds.

### P5 exit gate

- no stale architecture concept remains in documentation or machine-readable
  evidence;
- test-layout/review/security validators pass;
- production net growth is explained by a develop counterexample and invariant,
  not by migration residue;
- full nextest and clippy remain green.

P5 exit gate: **passed**. The remaining release blockers are the complete P6
process suite and the separately authorized P7 performance verdict.

## 7. P6 — Complete tx-pool integration acceptance

- Build once, enumerate every registered spec touching transaction submission,
  relay/proposal, RBF, reorg, persistence/restart, mining/template, compact
  blocks, pool RPCs and fee estimation—not only `test/src/specs/tx_pool`.
- Run through `make integration` in diagnosable deterministic batches and then
  as the documented complete impact universe.
- Classify each failure before editing: product bug, deliberately changed
  policy/stale test, harness isolation issue, or unrelated environment.
- Correct product defects only at the frozen authority/Plan/effect boundary;
  do not add adapter patches.
- Link every integration regression into `REVIEW_GUIDE.md`, the inventory and
  the security ledger.

### P6 exit gate

- the complete registered impact universe passes with no silent filtering;
- integration and unit/model behavior agree;
- failures and any exclusions have reproducible evidence.

P6 exit gate: **passed**. The final release binary passed all 150 managed
integration specs through `make integration` with `-c 1 --no-fail-fast` in
896.49 seconds. No spec was excluded. The final correction set also passed
230/230 nextest tests, clippy with warnings denied, and all review/security/test-
layout validators.

The closing hostile review confirmed and fixed definitive parent-loss
liveness, zero-match review-command anchors, production callback-timeout test
parity and the due-Ready prefix scan. It corrected the false later-arrival
claim, removed the genuinely dead ReadyKey size comparison, and recorded the
no-aging fee-priority trade-off for P7. The common no-dependent rejection path
retains direct allocation-free O(1) removal; cohort planning is paid only for a
real reverse-edge fan-out.

## 8. P6.5 — Exact admission transaction convergence

The pre-P7 whole-architecture review found one remaining encoding mismatch,
not a new lifecycle state: ordinary direct/pipeline commit waits on the global
largest-submit effect bound, performs expensive accepted/kernel planning under
the effect-journal mutex, and assembles dependency/template/effect publication
through separate caller discipline. This can serialize small admissions behind
unrelated queued effects and leaves the cross-authority proof distributed
across functions.

Implementation is constrained to one mutation family:

1. Add a concrete read-only `AdmissionPlan` containing the accepted
   `PoolMutationPlan`, matching total kernel handoff, exact immutable
   `EffectBatch` and plan-derived template delta.
2. Add a read-only pipeline terminal plan so rejected Ready work is parked or
   terminalized without invoking a fallible transition after Apply starts.
3. Perform every expensive/fallible computation outside the journal mutex.
   `try_apply` checks the actual batch before total kernel/pool/template Apply
   and append.
4. On `Full`, release all state locks, wait for the actual batch charge and
   replan. Delete ordinary `try_apply_bounded` and its post-Apply reset escape;
   retain the authoritative reorg/clear reset policy.
5. Preserve the six lifecycle states, accepted read concurrency, block-
   assembler reset/full priority, Local-direct behavior, RBF policy and public
   callback contract. No owner, undo log, reservation, universal event bus or
   reorg mega-plan may be introduced.

### P6.5 value gate

- the only ordinary PrePool→Accepted edge is statically visible in the
  `AdmissionPlan` API;
- every legal failure is mutation-free and no fallible kernel transition runs
  after the effect predicate accepts;
- effect charge is actual per admission, and RBF/capacity planning does not
  hold the effect-journal mutex;
- dependency and template deltas are derived from the same accepted plan,
  with no new manual publication point;
- focused failure/saturation/RBF tests, full nextest, clippy, validators and
  the managed integration universe pass before P7;
- production growth is accepted only where it shortens this proof or removes
  caller discipline; duplicate orchestration is a failed gate even if tests
  pass.

### P6.5 static-authority correction

The 2026-07-26 whole-tree review found 278 explicit production panic sites
(`assert*`, `expect`, `unwrap`, `panic!` or `unreachable!`) across 31 tx-pool
source files, compared with a much smaller inherited surface on `develop`.
Most new sites are not independent bugs: they expose one incomplete encoding.
`Entry` identity/state-derived values and mutable projection membership are
still duplicated, while `CohortPlan` proves their agreement only by convention.

The corrective implementation is one architectural change, not 278 rewrites:

1. State-specific, private constructors produce proof-carrying entries.
   Wait keys are non-empty by type; Ready identity/rank and residency charge
   are derived and cannot be supplied independently.
2. A prepared transaction owns the exclusive authority borrow from Plan
   through `commit(self)`. No mutation API is available between the read-only
   decision and the single-consumption total Apply.
3. One typed projection delta is generated from old/new primaries and is the
   only attach/detach path. Bespoke single-entry and queue-pop publication
   paths must converge on it or justify a strictly smaller typed equivalent.
4. Input/config/capacity/stale outcomes remain typed. Projection corruption is
   detected before mutation as a system fault and cannot be relabeled as a
   peer/RPC rejection. Apply contains no fallible operation and no panic site.
5. A validator rejects production `assert*`, `expect`, `unwrap`, `panic!`,
   `unreachable!`, unchecked indexing and unchecked arithmetic unless an
   explicitly reviewed process-boundary exception exists. Tests remain free
   to assert behavior.
6. Existing `catch_unwind` sites are audited under the same rule. Internal
   resolver, verifier, scheduler, publisher and authority code must propagate
   typed outcomes. Caller-supplied callbacks or other genuinely foreign code
   must be isolated by a thread/task/channel boundary; unwind catching must not
   drive tx-pool state, retry or recovery semantics.

The first AdmissionPlan liveness slice remains valid evidence (actual-owner
effect class, release-before-capacity-wait, exact journal outcomes and optional
history degradation), but P6.5 cannot exit until this static-authority gate and
its global review pass.

#### Frozen Rust data/API model

Implementation must follow this order; changing the model after editing call
sites is a design-gate failure.

1. **Primary entry.** A stored entry is constructed only by a private checked
   constructor. Its full hash is derived from the owned transaction, Wait owns
   a non-empty dependency type, Ready owns a non-empty bounded input type, and
   charge/rank/short ID are derived or carried by a constructor-produced
   validated wrapper. Sibling lifecycle modules may not assemble raw fields.
2. **Source identity.** Immutable remote ingress attribution (peer and declared
   cycles) and mutable scheduling authority (Remote/Proposal/Recovery) are
   separate typed values. A Remote scheduling owner is constructible only from
   matching remote ingress; promotion preserves ingress without duplicating a
   peer fact that later needs an assertion.
3. **Mutation set.** Cohort input is a private `EntryMutation` sum (insert,
   replace-exact-version, remove-exact-version). Insert/replace hashes derive
   from the entry. A bounded keyed builder prevents two final mutations for one
   hash before planning; callers no longer pass `(hash, Option<Entry>)` pairs.
4. **Prepared transaction.** Planning takes an exclusive authority borrow and
   returns `PreparedKernelMutation<'a>`. The type owns the primary delta,
   counter advances, dependency-level event and projection delta; only
   `commit(self)` can release the borrow. There is no separately callable
   public `apply_cohort` or mutation API between Plan and Apply.
5. **Projection delta.** One implementation derives every affected queue,
   waiter, Ready, peer, parent, deadline, short-ID, usage and active-work
   change from old/new primaries. Planning validates affected current
   projections and returns a typed `KernelFault` before mutation. Commit writes
   exact final membership with total collection operations; it does not use a
   fallible lookup, assertion, unwind or silent post-failure repair.
6. **Counters.** Entry version, arrival and dependency epoch are dedicated
   monotonic newtypes with checked allocation. All advances required by one
   mutation are reserved in its prepared transaction, so exhaustion cannot be
   discovered after primary or accepted Apply.
7. **Error sum.** The kernel error is an exhaustive tagged sum of transaction
   rejection, bounded backpressure, stale/duplicate outcome and `KernelFault`.
   Callers match the variant directly; classifier predicates and
   `pre_pool_reject` cannot relabel a system fault as a peer/RPC result.
8. **Boundary isolation.** Resolver/verifier code returns typed job settlement.
   Internal worker/publisher code has no unwind protocol. Genuinely foreign
   callbacks/endpoints run outside authority locks behind a task/thread/channel
   boundary, and only timeout/channel completion enters tx-pool state.

This model adds no lifecycle state, rollback log, repair generation or hot-path
lock. Newtypes, private constructors and exclusive borrows are zero-cost. The
projection plan remains bounded by the same affected cohort/edge limits; it
must not clone or rebuild an owner-wide or pool-wide projection on an ordinary
transition.

After implementation, perform the full architecture table in section 10 and
either accept the result as C17 or revert to C16. Benchmarking remains frozen
until explicit authorization.

### P6.5 implementation completion ledger

The working-tree candidate now implements the frozen model without adding an
owner, lifecycle state, rollback log, repair generation or hot-path lock:

- `StoredEntry` and state-specific private construction derive identity,
  source, rank, dependency and charge facts instead of accepting duplicate raw
  fields from lifecycle callers.
- `EntryVersion` and `Arrival` are checked monotonic newtypes. A private
  `MutationSet` produces an exclusive `PreparedKernelMutation<'_>` whose
  single-consumption commit owns every projection/counter change.
- Accepted `PoolMap` changes use exact sparse prepared mutations; status,
  graph, outpoint and aggregate contradictions return typed pre-mutation
  faults. Apply has no assertion, recovery or fallible policy decision.
- `AdmissionPlan` is the ordinary PrePool→Accepted protocol. Exact effect
  charge, dependency consequences, pool mutation, kernel handoff and block-
  assembler receipt derive from the same plan and linearize under the
  innermost journal predicate.
- Administrative remove, clear and reorg use typed bounded removal/generation
  plans. Clear atomically swaps authority with one explicit reset; reorg uses
  one chain-authoritative draft and a complete-generation fallback rather than
  replaying chain mutation or retaining a parentless suffix.
- Internal resolver/verifier/worker/publisher control flow returns typed
  outcomes. Production source is statically rejected for `assert*`, `expect`,
  `unwrap`, `panic!`, `unreachable!`, unchecked indexing/arithmetic and
  `catch_unwind`, and `clippy::await_holding_lock` is denied; only genuinely
  foreign endpoint code is isolated.
- Callback, network-ban and recent-reject database effects share the exact
  production timeout and stable endpoint circuits. A hung foreign call cannot
  pin the sole journal head; circuit opening permits at most one detached
  timed-out call per endpoint kind.
- Fresh success is journaled by `AdmissionPlan`; accepted-duplicate success
  holds a pool-membership read capability through append, so it cannot publish
  `Ok(old tx)` after a clear/reorg `GenerationReset`.
- Definitive terminalization—including the optional-history-full commit loser
  path—publishes bounded parent loss, so trusted Proposal/Recovery descendants
  cannot remain in `Wait(Missing)` forever.
- The unique cohort seal binds conflict history created by an Apply to that
  Apply's post-change dependency level. Older history can wake on the release,
  while the newly displaced victim cannot self-wake and restart an RBF cycle;
  callers do not publish or repair observation cuts manually.
- Block assembler concurrency is not collapsed into an actor: reset/full keep
  high-priority mutual exclusion, uncle/proposal/transaction partial work
  remains concurrent versioned OCC, and every successful full/reset
  replacement reissues all three dirty generations.

Current pre-P7 acceptance evidence:

- `cargo nextest run -p ckb-tx-pool --features internal`: 257/257 passed in
  24.829 seconds on the final working tree;
- `cargo clippy -p ckb-tx-pool --all-targets --features internal -- -D
  warnings`, `cargo fmt --all -- --check` and `git diff --check` passed;
- review/security generators validate 133 unique Rust anchors, 16 focused
  integration anchors and the complete 150-spec managed universe;
- the complete unfiltered 150-spec managed universe passed through its
  recorded serial `make integration` run, and the repository-wide unfiltered
  177-spec universe passed through plain `make integration` in 372.452 seconds;
- benchmark remains frozen and is the only open release gate.

### P6.6 review topology — mechanical only

- Kept `README.md` as the crate entry point, moved human design/review material
  into `docs/`, and moved tx-pool-owned validation tools into `scripts/`.
- Added a documentation index, a tool/CI usage guide, local-link/path drift
  validation, and a dedicated fast CI contract flow. Machine-readable JSON
  contracts remain in the crate root.
- Moved resolve/verify execution under `service/stages`, renamed stale
  coordinator vocabulary to `kernel`, and retired manager names where the code
  implements stage handlers rather than managers.
- Moved service-level scenarios and their harness out of `component/tests`,
  normalized `tests/mod.rs`, and replaced opaque `_seam` names with domain test
  or `_test_support` names. Shared builders are one crate-level `cfg(test)`
  support module; no production API visibility was widened.
- This phase changes module/file/test paths only. It adds no owner, state,
  branch, lock, await, allocation, capacity rule, failure outcome, or runtime
  publication edge. The discovered test count remains 257 and full nextest is
  green after relocation.

Physical Rust lines, with test roots/files and `benchmark.rs` excluded from
production, are 21,780 production / 55 files, 15,552 tests / 45 files and 1,470
benchmark / 1 file. The working candidate is 2,456 production lines smaller
than C1 but 3,248 larger than C13 and 14,483 above `develop`; it is 845 lines
above the P6.5 code checkpoint. That increase is accepted only provisionally:
it represents the typed prepared-state/projection proof, the removal of the
278-site runtime-panic proof surface, and the post-checkpoint peer-ban,
publication, dependency/event-cut and template-boundary corrections found by
the unfiltered integration audit. The final global audit found no new owner,
state, lock, undo/retry protocol or duplicated publication mechanism to remove;
test growth is reported separately and is not used to justify production
growth.

### Pre-benchmark audit disposition

The external pre-benchmark audit was checked against the working tree rather
than accepted as a change list.  Its publication findings split along existing
authority boundaries:

| Candidate | Verified disposition | Reason |
|---|---|---|
| ordinary submit status publication | covered by P6.5 | `AdmissionPlan` derives the status receipt from the same immutable pool plan and its sole Apply records the assembler delta; returning it from `PoolMap` would invert the component boundary |
| administrative `remove_tx` | completed in P6.5 | one typed bounded accepted/pre-pool removal reconciliation replaces manual cross-partition caller discipline without moving callbacks into `PoolMap` |
| generation reset pairing | completed in P6.5 | clear/reorg/fault paths use explicit service-level generation transactions; a closed journal rejects before authority Apply, and each caller retains its distinct typed payload rather than an untyped closure |
| reorg full reconcile exits | completed locally | reorg emits the authoritative reset and marks all uncle/proposal/transaction levels dirty in one refresh boundary; the later full plan derives uncle content from the bounded candidate authority and commits candidate cleanup only with its reset-epoch token, without changing full/reset/partial priority |
| malformed-peer ban ordering | completed in P6.5 | the expiring non-evicting marker intentionally linearizes before asynchronous network publication; immutable ingress removal, post-admission cleanup and the exact Ready-ticket fence cover queued/commit races, release every PrePool projection and preserve already-linearized Accepted state without a ban lifecycle state or an LRU eviction bypass |
| invariant `13 -> 8` renumbering | reject; group only | T1--T13 are useful review/evidence leaves.  They may be grouped under theorem families without renumbering 248 references or weakening independently checkable clauses |
| Tier A/C performance list | P7 candidates only | exact-tip, fast-path, clone and lock claims require code-specific safety proofs plus controlled A/B; charge caching is already implemented in `EffectBatch` and therefore is not an open change |

One immediately actionable observation belongs to P6.5 rather than a new
phase: `begin_next_commit` is read-only, so selecting a ticket must use the
kernel read boundary and must not run the mutation shell's derived-ready fanout.
The source carried by that ticket also determines the exact effect region;
Remote saturation must not block Proposal/Recovery work from trusted headroom.

No candidate above authorizes a universal event bus, service callbacks inside
`PoolMap`, a third transaction owner or a generic `before_apply` hook.

## 9. P7 — Performance and production acceptance

Execution is deliberately paused until the user's explicit benchmark
instruction. Correctness evidence is complete, but no performance superiority
claim is made before this gate.

- Audit benchmark setup, invariant assertions, warmup, variance and workload
  equivalence before trusting results.
- Compare clean worktrees/checkpoints for `develop`, C1 and final P6. Run quick
  repeated A/B first; expand only statistically inconclusive or regressed
  scenarios.
- Measure throughput, p50/p95/p99 latency, allocations/RSS, TxPool/kernel lock
  hold, worker utilization and commit/reorg/template latency.
- Throughput must not regress materially. A statistically significant
  regression blocks production even when correctness is green.
- Performance corrections may optimize projections/hints only; they may not
  add owner/state/undo protocols. Re-run correctness gates after each change.

### P7 exit gate

- no material throughput regression and no hidden latency/RSS/CPU regression;
- all P5/P6 gates remain green;
- final develop comparison proves necessity, safety, maintainability,
  extensibility, Rust-native failure semantics and residual risk.

## 10. Per-stage whole-architecture review

Every checkpoint answers all rows, not only the files edited in that phase.
The repository-root [`AGENTS.md`](../../AGENTS.md) Rust production checklist is a
required input to every pass, not merely an implementation-style hint. A
checkpoint cannot close while its type design, error model, ownership/borrowing,
async task lifetime, API misuse resistance, zero-cost abstraction or
maintainability review remains negative.

| Dimension | Required question |
|---|---|
| authority | Are `TxPool` and `PrePoolKernel` still the only executable payload owners? |
| state | Is every pre-accepted payload in exactly one of the six locations? |
| source | Are Remote/Proposal/Recovery attribution and Local-direct policy explicit and non-owning? |
| identity/ABA | Are raw hash, witness-hash cache key, short ID and monotonic version used only in their correct domains? |
| plan/apply | Can any ordinary failure occur after Apply begins or require rollback? |
| graph/wait | Are causal, conditional and availability relations distinct, bounded and level-triggered? |
| effects | Is every required stable outcome sequenced, charged and recoverable from endpoint saturation? |
| resources | Are every retained byte, active slot, peer share and graph fan-out bounded? |
| locks | Is a capacity hint released before `TxPool → kernel → journal`, with no reverse await/I/O under a state lock? |
| chain/template | Can reorg, Gap or uncle state strand a transaction or expose an old-parent template? |
| static Rust proof | Are invalid states prevented by private types/ownership first, with no production `assert*`, `expect`, `unwrap`, `panic!`, `unreachable!`, unchecked indexing/arithmetic, or `panic + catch_unwind` control path? |
| failure | Is every legal outcome typed before mutation, every genuine system fault detected before Apply, and genuinely foreign code isolated without selecting authority state, retry, rollback or recovery semantics? |
| attack | Can a peer turn legal input, saturation or a repeatable race into service-level DoS or unbounded work/residency? |
| performance | Is hot-path work bounded and allocation-conscious, with final claims deferred to A/B evidence? |
| compatibility | Does the change preserve consensus-visible determinism, database/on-disk compatibility, network/RPC behavior and a safe upgrade path, or document and test an intentional change? |
| evidence | Do unit/model/integration tests prove behavior rather than a deleted encoding? |
| Rust idiom | Does the complete diff pass every applicable root `AGENTS.md` review question, with compile-time/type/ownership guarantees chosen before runtime defense? |

Finding a problem is not permission to add a mechanism. First identify the
violated frozen rule, choose the smallest correction at its authority boundary,
and revert/reopen design if that rule itself is wrong.
