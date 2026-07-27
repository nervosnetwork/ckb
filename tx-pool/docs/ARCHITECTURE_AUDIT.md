# Independent Architecture Audit

Audited design: [`ARCHITECTURE.md`](ARCHITECTURE.md)

Execution ledger: [`IMPLEMENTATION_PLAN.md`](IMPLEMENTATION_PLAN.md)

Review evidence: [`REVIEW_GUIDE.md`](REVIEW_GUIDE.md)

Stable code checkpoint: `dd95e1f99`.
Audited candidate: post-regression correctness checkpoint `dd95e1f99`;
preceding P6.5 architecture checkpoint `9e559a482`.

## 1. Verdict

The two-authority Plan/Apply kernel is a coherent and materially stronger
architecture than `develop`. Its central improvement is not “more runtime
invariants”; it removes inferred pre-accept ownership and speculative accepted
mutation, the two premises behind most historical failures. The six-state
kernel, accepted immutable Plan/total Apply, bounded stable-effect journal and
level-triggered progress rules form one compatible model rather than a set of
local patches.

The P6.5 implementation and correctness acceptance are complete. The candidate
passed unit/model, all-target clippy, static/document validation and the same
complete unfiltered 150-spec managed process-integration universe as C15. Full
production readiness is not yet claimed because the separately authorized
controlled performance A/B remains open. Performance is a hard gate, not a
deferred nice-to-have.

## 2. Audit method

The review worked backward from `develop` counterexamples and the accumulated
security ledger, then traced current code through:

```text
network/controller ingress
  -> non-contextual validity and duplicate fence
  -> pre-pool admission/budgets
  -> resolve/wait/verify leases
  -> Ready selection
  -> accepted Plan/kernel handoff/total Apply/effect append
  -> relay/callback/template/RPC/persistence
  -> reorg/clear/shutdown
```

For every boundary it checked ownership, identity, legal outcomes, byte/work
bounds, lock order, lost-wake progress, endpoint failure and stale completion.
It also compared production/test `cfg` behavior and reviewed the simplification
diff for new owners, states, retries and rollback mechanisms.

## 3. Necessity review

| Design choice | Necessary? | Audit conclusion |
|---|---|---|
| two authorities | yes | one pre-pool authority closes inferred queue/orphan/worker handoffs; retaining accepted `TxPool` separately preserves concurrent reads and avoids a universal mailbox |
| six locations | yes | they are the minimum payload-shape phases; Missing/Conflict are one Wait reason, Recovery is source metadata, and no persistent commit/invalidated/undo state is required |
| one `u128` entry version | yes | closes stale lease/remove-readmit ABA without incarnation/revision dual plumbing |
| fair identity-only queues | yes | scheduling needs order and per-owner fairness, but queue payload ownership would violate the partition |
| accepted Plan/total Apply | yes | RBF and capacity eviction otherwise remove victims before every legal failure is known; nested undo creates a second mutation protocol |
| stable effect journal | yes | callbacks/relay cannot run under state locks, yet mutation must not lose its externally visible outcome under saturation |
| static trust partitions | yes | Remote saturation must not starve Local/Proposal or chain/admin convergence |
| one serialized Ready commit driver | yes | removes speculative `Committing`/`RaceLost` ownership; does not serialize resolve/verify work |
| level-triggered wait/template/reset state | yes | optimistic notification edges may be lost or coalesced; progress must derive from retained levels |
| generation swap on clear/over-bound chain recovery | yes, narrow | O(1) authority replacement prevents partial retained generations; retired payloads are dropped outside locks |
| fail-fast internal contradictions | yes, narrow | continuing after a primary/projection mismatch has no sound repair source; hostile legal paths must be typed before this boundary |

## 4. Alternatives rejected

### Harden the legacy queues

Rejected. Each additional queue-local fix still relies on a transaction being
absent from several stores between pop, await, park, clear and re-admission.
The ownership proof remains non-local and failures reappear as gaps, double
parking, ghost accounting or stale completion.

### Universal actor

Rejected for the production target. It gives a simple serial history but also
serializes accepted read/RPC/template traffic and makes mailbox residency and
large messages a new global bottleneck. The current design serializes only
mutations that truly cross authorities.

### Nested undo or snapshot rollback

Rejected. Undo must mirror every accepted/pre-pool index, counter, dependency,
victim and effect transition. A rollback failure needs its own state and tests.
Read-only Plan leaves the old state untouched on every expected failure and
has only one Apply implementation.

### Defect-domain restart / panic-and-catch control flow

Rejected. Internal resolver, verifier, scheduler, publisher and authority
paths use typed outcomes; private constructors and exclusive prepared plans
make invalid transitions unrepresentable. Genuinely foreign callbacks and
endpoints are isolated through thread/task/channel boundaries outside
authority locks. Restarting or repairing a kernel after an unwind requires
choosing truth from contradictory state and deciding which effects to replay;
catching that unwind does not make the choice sound.

## 5. Whole-architecture results

| Dimension | Result | Evidence/conclusion |
|---|---|---|
| ownership | pass | `TxPool` and `PrePoolKernel` are the only retained payload owners; derived queues/indexes are identity-only |
| state minimality | pass | six semantic locations; `WaitReason` and source do not multiply ownership states |
| ABA/identity | pass | exact full hash/version/location leases; witness-hash cache key; short ID is collision-aware index only |
| RBF/capacity atomicity | pass | complete policy and sparse capacity simulation in immutable Plan; Apply has no legal error path |
| dependency liveness | pass | exact reverse keys, bounded dirty levels, parent-loss demotion and final accepted revalidation |
| effects | pass in unit/model evidence | state Apply and bounded stable append share the innermost journal lock; foreign callback/network/database work occurs after locks behind timeout/circuits; accepted-duplicate success retains membership capability through append |
| reorg/admin | pass in unit/model evidence | chain mutation/recovery plan is one authoritative phase; clear uses generation swap; old generic replay retry removed |
| template liveness | pass | Gap reevaluation and uncle/proposal conflict filter; full rebuild derives from candidate-uncle authority and token-commits cleanup; Reset/full priority is preserved while uncle/proposal/transaction deltas remain concurrent versioned OCC and are all re-dirtied by replacement; both uncle-inheritance and ordinary-mining dependent-reorg process regressions passed |
| hostile input bounds | pass with one residual | entry/byte/peer/work/graph/effect/uncle bounds precede retention; trusted `NotifyTxs` vector length needs upstream proof |
| failure semantics | pass in static/unit evidence | legal hostile input is typed; production authority/worker code contains no explicit panic surface or unwind-driven control flow; foreign endpoints are isolated outside authority locks |
| test/production parity | pass after correction | candidate-uncle test-only limits and callback-timeout divergence removed; remaining `cfg(test)` sites are wiring/observation/fault seams tracked by manifest |
| simplification | pass for P5 code slice | redundant entry clones, disposal wrappers, submit envelopes, duplicate reorg plan and assembler loop removed without new state |
| integration | pass | P6.5 passed the complete unfiltered managed 150-spec universe through `make integration` with `-c 1 --no-fail-fast` in 884.185 seconds and the repository-wide unfiltered 177-spec universe through plain `make integration` in 372.452 seconds; RBF cycling, uncle inheritance and ordinary-template dependent-reorg regressions passed in the latter run |
| performance | open/blocking | no superiority claim before controlled checkpoint A/B and lock/allocation/tail-latency analysis |

## 6. Static Rust proof audit

The final ordering is mandatory:

```text
private type/ownership proof > typed pre-mutation result > foreign-code isolation
```

Production transaction, authority, worker and publisher paths may not use
`assert*`, `expect`, `unwrap`, `panic!`, `unreachable!`, unchecked indexing or
unchecked arithmetic as correctness mechanisms. They may not use
`panic + catch_unwind` to choose settlement, retry, rollback, shutdown or
generation state. The crate also denies `clippy::await_holding_lock`, matching
the documented ownership rule with a compile-time gate. Startup/configuration
failures return startup errors;
malformed, stale, duplicate, capacity, RBF, clear and shutdown outcomes remain
typed. Genuinely foreign callbacks/endpoints are isolated outside authority
locks and report channel/timeout failure.

This does not promise recovery from OOM, abort, FFI corruption or arbitrary
memory corruption. It does require legal inputs and internal protocol outcomes
to stay within statically or explicitly typed control flow. A structural fault
that cannot yet be made unrepresentable must be detected before mutation as a
typed system fault, never asserted during Apply or caught afterward.

The current legal-input/fault separation was re-audited by transition class:

| Boundary | Legal outcome | Why it cannot become a structural fault |
|---|---|---|
| fresh ingress | typed rejection/backpressure/duplicate before ownership | `PrePoolAdmissionError` is a closed adapter domain; malformed or capacity input never enters the fault arm |
| resolve/verify completion | stale is discarded; charge/shape growth is an explicit public rejection | both phase-growth branches terminalize the exact lease and publish Reject; focused regressions prove the generation remains live |
| resolver/verifier checkout | executable lease or no work | queue selection and active-owner capacity are evaluated under the same kernel borrow; no peer-controlled fact is introduced by checkout |
| parent wait/wake | park, requeue, public rejection or stale | discovered dependencies are bounded before Apply; every resident owner reserves its maximum Wait metadata, so a wake cannot discover new capacity pressure |
| expiry, ban and definitive terminalization | bounded prepared removal/demotion | these transitions only remove ownership or move dependents into the continuously reserved Wait shape; immutable ingress attribution is rebound under authority before peer removal, while post-admission and exact Ready-ticket fence checks close both sides of the ban race without adding a lifecycle state |
| Ready handoff | policy/backpressure before Apply, then total settlement | the global conflict cohort is bounded explicitly, optional history degrades to terminalization, and ticket selection plus Plan/Apply share one kernel borrow inside the accepted-pool sequencer |
| reorg recovery | closure-safe admitted prefix or generation reset | an empty scratch generation proves the exact prefix under identical limits before the real empty generation consumes it |

`PrePoolError` remains the shared private planner error because the same bounded
cohort compiler serves ingress and authority-only transitions. The remaining
`into_unexpected_fault` sites therefore carry a review obligation rather than
a new runtime state: they are permitted only at the authority-only rows above.
Adding a public error variant, changing conservative charge equations or adding
another call site requires re-running `TP-DEFECT-001` and either proving the
row still excludes that outcome or replacing the boundary with a narrower
operation-specific result type. This maintenance risk is recorded as O18; it
is not evidence of a currently reachable legal-input fail-stop path.

### Regression convergence and architecture-drift review

The unfiltered process suite found failures which narrower unit and managed
integration runs did not expose. They were reviewed together before accepting
their fixes:

| Regression | Root boundary | Architectural correction | Model cost |
|---|---|---|---|
| reverse/unordered orphan relay | resolution published one observed miss rather than the complete bounded direct frontier | build and settle the exact frontier under the existing `TxPool -> PrePoolKernel -> EffectJournal` boundary | no owner, state, lock or retry added |
| Local test submission | an API source was routed by pipeline convenience rather than its synchronous contract | keep `Local` structurally outside the pre-pool and reuse ordinary direct Plan/Apply | one source-policy correction; no new mechanism |
| detached-uncle inheritance | reset/full copied a transient derived template and optional uncle retention could lose the publication race | derive full content from candidate authority and commit cleanup only with the matching reset/publication token | two typed template counters replace a lock-held construction window; partial work remains concurrent |
| RBF cycling | a new conflict victim observed the pre-Apply dependency epoch and consumed its own replacement release | the unique cohort seal binds only same-Apply conflict waiters to the post-Apply observation cut; old history still observes the edge | one narrow consuming `StoredEntry` transform; no caller-managed publication |
| suite-wide RPC timeout | loopback integration RPC inherited desktop proxy discovery | make the integration client explicitly no-proxy | harness-only failure-domain isolation |

These failures cluster at facts crossing an authority boundary: complete input
frontiers, API source policy, derived publication tokens and event observation
cuts. They are not evidence that another lifecycle location or a rollback
engine is missing. The corrective rule is now explicit: every Plan must carry
not only its final values but also the exact observation/publication cut at
which derived work becomes valid. That cut is sealed once by the owning
authority; callers may not reconstruct it from pre-Apply snapshots or
post-Apply notifications.

The review found no model drift after these corrections. Production still has
exactly two payload authorities, six pre-pool locations, one accepted Apply,
one kernel cohort seal, one stable-effect journal and no undo/restart protocol.
The remaining risk is transferred only where it is already declared: typed
system faults still stop an inconsistent generation (O18), endpoint delivery
is bounded rather than exactly-once, and the new bounded work under the pool
and kernel locks has not yet received the P7 performance verdict. Replacing
these residuals with a universal actor, event bus or recovery state would add
more failure modes than it removes.

## 7. Attack review

### Defended classes

- duplicate/hash/short-ID/witness-variant aliasing;
- stale lease and remove/re-admit ABA;
- Remote global/per-peer memory and active-work exhaustion;
- dependency fan-out and multi-input conflict-product blowup;
- under-fee/free replacement and RBF victim churn;
- effect FIFO/headroom starvation and relayer saturation;
- callback panic/hang reentrancy;
- parent/child stranding after commit, removal, reorg or clear;
- recovered Gap/uncle proposal censorship;
- persistence of a generation after an invariant failure;
- test-only scheduling limits masking production behavior.

### Residual attack/operational risks

1. `NotifyTxs(Vec<_>)` relies on a trusted upstream controller for batch-size
   admission; each element is bounded, the outer allocation is not proven here.
2. A genuine typed pre-Apply primary/projection contradiction still requests a
   controlled stop and skips persistence. The static/type boundary prevents a
   legal peer outcome from selecting it; it is not a repair/restart protocol.
3. Effect delivery is bounded/reconcilable, not exactly-once; an unavailable
   endpoint can lose detail after generation coalescing. A timed-out blocking
   endpoint can leave one detached foreign call until that call returns, after
   which its stable circuit suppresses further calls of that kind.
4. A shutdown persistence filesystem call can still block its save task; it
   does not hold live tx authority but remains an operational timeout concern.
5. Process OOM/abort and crash-durable persistence are outside the model.
6. CPU/RSS/tail-latency resistance under realistic multi-peer stress awaits P7.
7. Block-template HTTP notification futures are timeout-bounded, but an
   operator-configured non-terminating notify script has no explicit
   child-process kill-on-drop proof. This inherited configuration boundary is
   tracked as O14 and does not hold transaction authority.

None warrants a new owner or recovery protocol now. They remain explicit gates
or documented operational limits.

## 8. Maintainability and extensibility

The architecture is maintainable because each new behavior must select one of
two owners, one of six locations, one bounded command and one effect boundary.
Derived views can be rebuilt in tests from primaries. A reviewer need not infer
location from queue membership or reason about undo after every failure.

The main remaining review burden is volume, not conceptual fragmentation.
Using physical Rust lines with `tests/`, `tests.rs` and `benchmark.rs` reported
separately (production files contain no inline test body), the current result is:

| Tree | Production | Tests | Benchmark |
|---|---:|---:|---:|
| `develop` | 7,297 / 23 files | 2,434 / 13 files | 0 |
| C1 `02e648255` | 24,236 / 60 files | 20,808 / 55 files | 1,422 / 1 file |
| C13 `6d0577ad4` | 18,532 / 53 files | 13,602 / 44 files | 1,463 / 1 file |
| P6.5 checkpoint `9e559a482` | 20,935 / 54 files | 14,429 / 44 files | 1,470 / 1 file |
| post-regression review-layout candidate | 21,780 / 55 files | 15,552 / 45 files | 1,470 / 1 file |

The current candidate remains 2,456 production lines smaller than the audited
C1 intermediate architecture, but adds 3,248 lines over C13 and remains 14,483
above `develop`. It is 845 production lines above the P6.5 checkpoint; tests
grew separately by 1,123 lines. That growth is material. The C13→current cost is
concentrated
in proof-carrying stored entries/prepared projection changes, exact accepted
plans, typed administrative generation boundaries and stable-effect coupling;
the post-P6.5 increment closes peer-ban admission races, legal-input/fault
classification, stable endpoint and accepted-result publication, dependency
loss/event-cut liveness, and template publication races. It replaces the
278-site runtime-panic proof surface rather than adding a new owner, state,
rollback or restart protocol. Test growth is reported separately and does not
excuse production growth. The final call-site review found no duplicated owner,
publication or observation mechanism whose deletion preserves the same proof;
P7 may still justify local allocation or lookup optimizations. Collapsing typed
phase payloads, Plan/Apply, exact wait keys or effect coupling would weaken the
proof.

Extension is intentionally constrained. A seventh payload state, third owner,
rollback journal, broad retry, reverse lock acquisition, unbounded graph or
test-specific production policy requires reopening the architecture rather than
being merged as an implementation detail.

## 9. Acceptance gates

P5:

- machine contract, review registry, test-layout and security validators match
  the actual six-state/Rust failure model;
- production/test/benchmark line counts are reported separately;
- all `ckb-tx-pool` `nextest` and all-target clippy pass.

P6:

- enumerate and run the complete managed tx-pool process impact universe via
  `make integration`, not only `test/src/specs/tx_pool`;
- classify every failure as product defect, intentional behavior/test drift,
  harness isolation, or environment before editing;
- link new regressions to the review guide and inventory.

P7:

- run clean checkpoint A/B after benchmark-harness noise review;
- no material throughput, tail-latency, CPU, RSS/allocation, lock-hold, reorg or
  template regression;
- rerun correctness gates after any performance correction.

## 10. Final audit position

The architecture is preferable to `develop` and its additional mechanisms are
necessary at the root boundaries they protect. It is also substantially
simpler than the intermediate coordinator/undo/defect-restart designs.
Post-regression static and process correctness acceptance is complete; the
correct next step is the separately authorized performance verdict, not
another redesign. Any new problem must first be mapped to an existing
authority/invariant; only a contradiction in those frozen rules justifies
changing the model.
