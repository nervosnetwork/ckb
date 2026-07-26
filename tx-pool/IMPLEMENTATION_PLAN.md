# Tx-Pool Root-Cause Refactor — Live Execution Plan

Design authority: [`ARCHITECTURE.md`](ARCHITECTURE.md)
Independent audit: [`ARCHITECTURE_AUDIT.md`](ARCHITECTURE_AUDIT.md)  
Review evidence: [`REVIEW_GUIDE.md`](REVIEW_GUIDE.md)

Status date: 2026-07-26
Current stable checkpoint: `288031ebc`
Current phase: P7, controlled performance acceptance (awaiting explicit authorization)

This file is the live execution ledger. It is updated at every checkpoint so a
compaction or restart can resume from the actual code rather than an obsolete
design stage. The architecture, audit, pipeline, review registry, test-layout
and security documents were migrated to the final six-state/Rust-native model
in P5; any historical mechanism named by the security ledger is evidence, not
current implementation authority.

## 1. Frozen direction

The refactor is allowed to retain only concepts needed to prove a develop bug
or a stated security/performance boundary.

1. There are two executable transaction authorities: accepted `TxPool` and
   pre-accepted `PrePoolKernel`.
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
   trusted or chain-critical headroom; a saturated detail path converges via a
   constant-size `GenerationReset` authority.
7. Performance is a hard acceptance criterion. Hot single-entry transitions
   keep stack-sized plans; the heap-backed cohort planner is reserved for
   bounded multi-entry transitions. No benchmark runs before P7.
8. Production line counts exclude tests and benchmarks. There is no arbitrary
   line target: growth must be justified by removed develop failure modes and
   measured performance, while every safe deletion remains required.

## 2. Rust-native failure contract

The final develop comparison must audit this contract explicitly.

| Outcome | Encoding | Service effect |
|---|---|---|
| malformed/policy rejection | typed `Reject`/`PrePoolError` | reject or ban according to policy; service remains live |
| bounded capacity/backpressure | typed `Result` | no mutation or bounded optional-history degradation |
| stale asynchronous lease/race | typed stale/duplicate outcome | discard, retry the level, or preserve the exact current owner |
| shutdown/channel/config/resource outcome | typed at its external boundary | controlled shutdown or startup failure |
| untrusted resolver/verifier/callback/endpoint panic | catch only at the computation/side-effect endpoint | terminalize/quarantine the owned job; no state repair protocol |
| primary/index/accounting contradiction | assertion/`expect` inside the authority boundary | fail-fast, cancel workers, and skip persistence |

The target is not an Erlang-style panic-free or generation-restart framework.
Legal hostile input must not reach the last row; truly impossible internal
states do not travel through the same recoverable error channel as input.

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
- Preserved block assembler semantics: Reset/update_full serialize on the full
  template boundary, update_full has priority, and optimistic proposal/
  transaction deltas remain level-triggered.

## 5. P4 — Complete: failure semantics and authority acceptance

### Completed

- Removed `PrePoolError::Repair` and the generic Defect error class.
- Converted primary/index/accounting contradictions to assertions inside the
  kernel authority; expected rejection, capacity, duplicate and stale outcomes
  remain typed.
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
      proposal/transaction generations dirty, while later chain deltas keep
      flowing. Remaining retries are capacity-notified or level-triggered
      external/derived work; no authoritative mutation is replayed.
- [x] Complete the call-graph proof. A capacity hint takes and releases the
      journal lock first; nested mutation then follows
      `TxPool → PrePoolKernel → effect journal`, with no reverse await or I/O
      under a state lock. Kernel-only worker transitions are the sole shorter
      form.
- [x] Verify every cross-authority pool plan settles kernel ownership before
      accepted Apply. Expected policy/capacity decisions finish in Plan;
      coordinator operations after Plan are continuous-reservation/identity
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
  hard full-template barrier and ordinary proposal/transaction updates as
  level-triggered optimistic deltas. Update queues move rather than clone.
- Removed production `PipelineState::chunk_rx`; worker receivers belong to the
  worker runtime and direct white-box observation belongs to the test harness.
- Candidate-uncle tests now exercise production limits (128 candidates/10
  selected) instead of changing scheduling behavior under `cfg(test)`.
- Deleted stale multi-queue/orphan terminology at production boundaries.

### Completed architecture/evidence acceptance

- Recounted physical Rust lines with tests and benchmark excluded from
  production: C13 has 18,532 production lines versus C1's 24,236 (−5,704) and
  develop's 7,297 (+11,235). Tests are 13,602 lines and benchmark is 1,469;
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

## 8. P7 — Performance and production acceptance

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

## 9. Per-stage whole-architecture review

Every checkpoint answers all rows, not only the files edited in that phase.

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
| failure | Is the outcome typed when legal, caught only at an untrusted endpoint, and fail-fast only when structurally impossible? |
| attack | Can a peer turn legal input, saturation or a repeatable race into service-level DoS or unbounded work/residency? |
| performance | Is hot-path work bounded and allocation-conscious, with final claims deferred to A/B evidence? |
| evidence | Do unit/model/integration tests prove behavior rather than a deleted encoding? |

Finding a problem is not permission to add a mechanism. First identify the
violated frozen rule, choose the smallest correction at its authority boundary,
and revert/reopen design if that rule itself is wrong.
