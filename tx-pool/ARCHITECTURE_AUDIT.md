# Tx-Pool Root-Cause Redesign — Independent Architecture Audit

Audit subject: [`ARCHITECTURE.md`](ARCHITECTURE.md)
Code checkpoint: `35cabc9b7` plus the preserved pre-redesign correctness/test
working tree  
Audit result: **GO with explicitly recorded non-blocking residual risks**  
Benchmark result: **not run; final performance gate remains open**

This is the read-only scheme audit required by `ARCHITECTURE.md` section 23. It is
not evidence that the target has already been implemented. A design finding was
allowed to change the frozen document only once as part of the concentrated
revision below; the full gate was then restarted against that revision.

## 1. Evidence calibration

| Fact | Audited result | Consequence |
|---|---:|---|
| current production Rust under `tx-pool/src`, excluding tests and benchmark | 24,236 lines | current checkpoint is not the minimal proof surface |
| `develop` production Rust under the same definition | 7,297 lines | target must delete materially, not wrap HEAD |
| current direct production `TxPool.read/write().await` acquisitions | 34 in 11 files | a single global actor/snapshot is not justified; the regex definition is recorded instead of repeating 17/40/52 estimates |
| historical ledger findings | 152 | these are evidence cases, not 152 independent mechanisms |
| ledger disposition | 112 Covered; 21 Superseded; 16 Covered/O5 deferred; 3 Accepted | no Open/Partial historical row is hidden by the redesign |
| review behavior registry | 15 behaviors, 86 unique unit anchors, 10 integration anchors | P0 must remap them to the target model before deleting old encodings |
| default accepted/pipeline residency | 1,000,000,000 / 384,000,000 bytes | the target has enough existing configuration surface; sublimits must sum inside it |
| transaction ingress / block byte limit | 512,000 / 597,000 bytes | raw wire size is not a sufficient resident/scratch charge |
| default network peers | 125 | useful attack calibration, but not a tx-pool Sybil guarantee |

The 34-site guard count uses direct `tx_pool.read().await` and
`.tx_pool.write().await`-shaped production accesses. Wrapper-level logical
operations can produce a different number. It is topology evidence, not a
latency measurement.

`develop` was re-read rather than inferred from current tests. It has separately
locked `VerifyQueue`, `OrphanPool` and `TxPool`; a worker pops ownership before
fallible work; duplicate admission checks cross those stores non-atomically;
RBF physically removes victims and publishes effects before every later step
succeeds; queued/active/resolved memory is not one continuous budget; conflict
recovery is transferable; reorg/save do not have one charged retained owner;
and detached replay looked up a witness verification cache with a raw hash.
Those are concrete reasons to leave `develop`, independent of HEAD's size.

## 2. Concentrated blocker disposition

| ID | Falsifier found during audit | Classification | Concentrated correction |
|---|---|---|---|
| A-B1 | `Ready` had no normative total order, so conflict eligibility and commit CAS could drift | design blocker, closed | freeze one `ReadyKey`; remove `Committing`; define expiry/starvation semantics |
| A-B2 | Remote admission was described as kernel-only and could not atomically prove accepted absence | design blocker, closed | admission is `TxPool read -> kernel`; reject an accepted-hash mirror |
| A-B3 | a Resolve result could install dependency edges after the removal event it needed to observe | design blocker, closed | complete Resolve while retaining its Pool read, then take kernel |
| A-B4 | swapping an entire kernel would discard already committed, unpublished effects and could reuse clocks | design blocker, closed | stable kernel shell owns journal/clocks/DefectDomain; swap only entry/index generation |
| A-B5 | a FIFO critical slot could be full when an authoritative reorg needed to linearize | design blocker, closed | replaceable latest-authority register with constant-size `GenerationReset` |
| A-B6 | deleting persistent reader→spender ancestry alone left spender-first dep admission pinned | design blocker, closed | role-aware resolver ignores pool consumer for deps; selected-set conditional sort handles either arrival order |
| A-B7 | persistent Local promotion could outlive a cancelled public RPC and remove its deadline | design blocker, closed | Local is only a charged synchronous borrower; Proposal is the only retained trust promotion |
| A-B8 | member-count accounting ignored retained `HashMap`/`Vec` capacity after churn | design blocker, closed | compact immutable slices; charge mutable allocated capacity; bounded shrink/generation replacement |
| A-B9 | a delayed reset hint could leave an old-parent template visible during recovery | design blocker, closed | every returned template validates `(chain_generation,parent_hash)`; notifications remain hints |
| A-B10 | a proposed per-connected-peer base reserve required a new network-session ownership protocol yet still did not solve connection Sybil | overdesign, removed | retain global/per-peer bounds and fair admitted work; record sustained Remote saturation honestly |

After these changes, the audit restarted from the authority/state product rather
than assuming each local correction composed safely.

## 3. Restarted hard-gate audit

| Section-23 gate | Result | Evidence / falsification argument |
|---|---|---|
| exactly two executable authorities | PASS | accepted `TxPool`; asynchronous unaccepted `PrePoolKernel`. Local/worker/save/template/disposal/effect objects are charged borrowers or non-executable records. ConflictCache and handler replay ownership are deleted. |
| no persistent commit/invalidation/undo protocol | PASS | serial plan attempt + final version/rank CAS; one Wait; immutable PoolMutationPlan; generation reset is failure containment, not rollback |
| no ordinary fallible step after Apply | PASS at design level | policy, arithmetic, projection, capacity, effects and allocation reserve occur in Plan; Apply moves prepared values. Unknown Apply panic swaps ephemeral generations before guards open. Implementation fault injection is mandatory. |
| no legal/hostile input to service fail-stop | PASS with process-level exclusions | typed Plan outcomes cover input; worker/Plan/callback unwind is local; Apply unwind uses DefectDomain. OOM abort, double panic, FFI/process abort remain outside in-process proof. |
| bounded work/state before work | PASS at design level | closed entry charge, allocated-capacity charge, ingress/work/plan/save/template/disposal permits, one cohort cap and static effect regions all belong to two configured envelopes. Exact constants remain P0/P1 acceptance work. |
| concurrent Pool reads/template priority retained | PASS | resolver retains concurrent Pool read; no O(pool) snapshot per Apply; template lock preserves reset/full exclusion and full priority, plus a generation check on every returned template |
| historical behavior coverage | PASS as migration obligation | all 152 rows already name I1-I12; section 4 supplies a total bridge to F1-F8/target invariants. P0 rewrites executable anchors before old tests can be deleted. |
| code-size convergence before optimization | PASS as hard implementation gate | target ≤14k is an escalation envelope; every phase is net-negative; the optional fused scheduler path is not part of cutover |
| performance gate honest | PASS statically; measured gate OPEN | conservative path counts 10 authority acquisitions versus develop's nominal 7, but removes current reconciliation. No measured superiority is claimed before checkpoint/develop A/B. |

## 4. Complete historical mapping bridge

The ledger's 152 rows map to old invariant IDs with these occurrence counts:

| Old evidence invariant | Rows | Target root family / controlling target invariant |
|---|---:|---|
| I1 one lifecycle owner | 44 | F1; Partition, Lease, Budget |
| I2 authoritative commit | 13 | F2; Atomic acceptance |
| I3 rollback | 6 | F2; replaced by mutation-free Plan + total Apply |
| I4 no silent loss | 59 | F1/F3/F5/F6; Partition, Stable effects, Level-triggered progress |
| I5 bounded state | 49 | F4; Budget, Bounded hostility |
| I6 dependency events | 26 | F5; Wait exactness, Level-triggered progress |
| I7 chain transitions | 27 | F7; Chain serialization, Template authority |
| I8 stable effects | 21 | F3; Stable effects, Critical schedulability |
| I9 conflict scheduling | 28 | F2/F6; Ready exactness, Atomic acceptance |
| I10 pool graph | 20 | F2/F8; Atomic acceptance, rebuildable projections |
| I11 template liveness | 14 | F7/F8; Accepted status exactness, Template authority |
| I12 performance | 32 | F4/F7; Bounded hostility plus the still-open measured gate |

Rows can name multiple old invariants, so counts intentionally sum above 152.
The bridge is total because every historical row already has at least one
I1-I12 target. P0 makes `root_families` and target behavior IDs machine-readable
and changes CI to reject a row without both mappings. This keeps 152 attack
traces without encoding 152 states or mechanisms.

## 5. State/command product audit

The one state enum is:

```text
RecoveryRetained
ResolveQueued | ResolveLeased
Wait
VerifyQueued | VerifyLeased
Ready
```

For each state, the audit checked admission/promotion, checkout, stale and
current completion, expiry/admin removal, chain availability change, same-hash
Local finalization, Proposal witness replacement and generation reset.

Key conclusions:

- every queued/leased pair differs only because a borrower must be versioned;
  merging them would lose cancellation/ABA safety;
- `Wait.reason` changes policy/telemetry, not ownership or executor;
- no command needs `Committing`, `Invalidated`, `RaceLost` or a conflict owner;
- Local does not add a state because its caller remains the bounded borrower;
- a global non-reused `u128` version plus expected state/rank/deadline makes an
  old lease/task inapplicable after every reuse-relevant transition;
- entry-generation swap never resets that version or the effect sequence;
- version exhaustion cannot wrap and has an orderly operational outcome.

The target reference model must enumerate small command/state combinations and
recompute all indexes/charges after each command. It is test-only; production
uses touched checks and the DefectDomain.

## 6. Lock and linearization audit

Only this nested authority edge exists:

```text
optional bounded permit -> TxPool read|write -> PrePoolKernel
```

No kernel guard awaits or acquires TxPool. Resolve holds the Pool read it
already needs and completes into the kernel before opening that guard. Verify
does not hold either authority during scripts. The commit serial permit owns no
payload and is released without an authority guard wait. Effect endpoints,
filesystem, network, callbacks and assembler locks run after state locks open.

Assembler is a separate acyclic domain: `template_lock -> TxPool read`; state
mutation only updates the stable effect register. Candidate-uncle state is
released before the template lock. The save-writer mutex is acquired only after
the coherent in-memory snapshot has been copied and no mutation path acquires
it.

The design deliberately accepts one extra Pool read at Remote admission. Any
attempt to remove it needs a pool-sized accepted identity projection or permits
temporary double ownership and is therefore rejected.

## 7. Resource and attack audit

| Attack/failure | Bound and response | Residual |
|---|---|---|
| tiny malformed flood | ingress count/bytes, per-peer/global Remote, fixed fair workers, noncontextual work after admission | attacker can consume its configured fair share repeatedly |
| huge dep/dep-group expansion | budgeted provider extends work permit before allocation; per-job/global cap returns typed resource outcome | legal work above configured support limit is best effort/rejected |
| worker cancel/panic after owner removal | work permit covers borrower until final Arc drop; version makes completion stale | process abort excluded |
| witness churn | one raw-hash owner; untrusted alternate witness cannot replace active work; wtx-keyed bounded fingerprinting | Proposal replacement can consume its trusted bounded reserve |
| Remote Sybil saturation | per-peer/global count+bytes and fair admitted work; attacker identity floor exposed by config | no honest Remote admission guarantee under enough connection Sybils; Proposal/Local/chain unaffected |
| popular Wait key | ordered `(hash,version)` bucket, epoch cursor, pending follow-up, fair slices | delayed but bounded per slice |
| repeated availability churn | coalesced key epoch and final-state availability; no transient free event | attacker can consume configured maintenance share |
| high-fanout accepted removal | bounded ordinary cohort; authoritative over-bound event resets ephemeral generation | valid block can cause observable mempool loss |
| RBF/capacity overlap | one deduplicated physical union and cap; sparse overlay; every rejection mutation-free | very high-fee candidate may be rejected if fitting needs too many tiny removals |
| effect sink full/hung | Remote ceiling/trusted headroom; endpoint timeout/circuit; stable journal survives kernel swap | at most one charged non-cancellable blocking task per validated endpoint |
| chain effect while ordinary full | replaceable constant-size latest authority; optional detailed ordinary batch degrades to GenerationReset | per-item diagnostics may be coalesced |
| allocator-capacity churn | charge mutable container capacity; no refund until shrink/rebuild | allocator RSS calibration remains benchmark work |
| repeated latent Apply defect | one DefectDomain, prebuilt spare, one DisposalPermit, Remote Closed/Cooling/Open | repeated defects can disable Remote while trusted/chain/query stay alive |

The memory proof is two checked envelopes, not an RSS equality claim. Every
sub-limit, active borrower maximum and emergency/disposal capacity must sum
inside the existing accepted and pipeline resident configuration. If an
indivisible proposal/plan/effect/save item cannot fit, startup rejects the
configuration or the optional operation has a documented typed failure; a
chain command uses the reset fallback.

## 8. Pool policy and graph audit

The immutable PoolMutationPlan is necessary: `develop` and HEAD discover RBF,
cell-ref, insertion, size and effect failures after physical removal. Nested
undo is not independently necessary once all decisions are pure facts in one
top-level plan.

The sparse virtual eviction overlay survived falsification only after adding
the candidate's accepted ancestors to the adjusted frontier. A frozen initial
key would change CPFP policy; a full PoolMap clone is O(pool); physical probing
reopens lost-victim bugs. Differential comparison with the current stepwise
policy is therefore a deletion prerequisite for the old journal, not evidence
that the overlay may be skipped.

The accepted graph contains only causal producer dependencies. Cell-reference
reader→spender is role-aware conditional template order. Both admission orders
are allowed; selected cycles are valid pool states and deterministically shed a
weak member only from that template. This removes the fee-free mass-eviction and
spender-first pinning surfaces without weakening consensus validation.

## 9. Maintainability, extensibility and static performance

The target is cohesive rather than a collection of patches:

- ownership changes add a closed state transition;
- scheduling changes add only an exact derived key;
- a dependency feature adds one typed key/resolver rule;
- accepted removal authority adds one PoolMutationPlan cause/proof;
- an endpoint adds one bounded effect variant/failure policy;
- failure containment remains one DefectDomain.

No extension is allowed to add a payload store, transferable ticket, second
transaction protocol, executable cache or another candidate order. The stable
kernel shell and swappable entry generation are one authority, not two models.

The conservative success path has 10 authority acquisitions versus develop's
nominal 7 and current HEAD's at least 16 under the documented counting method.
It retains concurrent resolution/verification and avoids pool-sized snapshots,
global scans, generic undo and current conflict/capacity reconciliation. This is
performance-feasible, not a measured win. Direct Resolve→Verify lease and Ready
hints remain forbidden until the final A/B attributes a real regression to
these acquisitions.

The 14k production-line target remains an escalation envelope. Reviewability is
primarily enforced by deleted mechanism names, one state/order/equation set and
net-negative phases; line compression may not hide branches or merge unrelated
responsibilities.

## 10. Recorded residual risks

| ID | Accepted non-blocking risk | Why it does not invalidate GO | Required evidence/operation |
|---|---|---|---|
| R1 | sustained Remote admission denial by enough connection Sybils | all memory/CPU is bounded and trusted/chain progress is isolated; tx-pool cannot solve connection identity | expose identity floor/saturation; relay retry; network-layer mitigation may be separate |
| R2 | an over-bound valid chain event can reset unrelated mempool contents | mempool is ephemeral; alternatives are unbounded lock work, partial invalid state or another owner/protocol | integration fault case, reset metric, latest template and persistence-skip proof |
| R3 | current selector's bounded non-fitting-prefix cap can underfill a block | bounded availability trade already recorded as O6; removing it reopens O(pool) work | retain until a resumable fit-aware design passes A/B |
| R4 | OOM/allocator abort, panic-abort build, FFI abort or double panic escapes in-process containment | Rust cannot recover these safely | prod profile unwind CI assertion; process supervision |
| R5 | unexpected crash before explicit atomic save loses accepted/recovery mempool work | existing mempool durability is best effort; WAL/fsync is a separate product requirement | v2 orderly save/restart tests and documentation |
| R6 | first accepted full projection repair can pause readers up to accepted-limit work | rare defect-only path avoids a normal-path lock/snapshot protocol; recurrence resets/gates | repair latency metric/fault test; benchmark hostile fault separately if needed |
| R7 | exact default sublimit calibration and allocator safety margins are not yet measured | design equations are complete but constants are implementation/performance evidence | P0/P1 checked derivation and final RSS A/B |

## 11. Authorization and stop conditions

The revised Thin Transaction Kernel plus per-authority Plan/Apply design is
authorized for implementation. The global actor, patch-develop, and
wrap-current-coordinator alternatives remain NO-GO.

Implementation must stop and return to architecture audit—rather than add a
local compensating mechanism—if any phase needs:

1. a third retained executable payload owner;
2. a new persistent state outside the seven-variant enum;
3. a second candidate order or entry ABA token;
4. mutation rollback/nested undo or a fallible ordinary Apply;
5. a reverse authority lock edge or lock held across I/O/work;
6. a population hot-path scan or an uncharged borrower;
7. an input-triggerable service fail-stop;
8. production source above the reviewed 14k envelope without a new necessity
   proof;
9. a measured correctness/performance regression hidden by weakening tests or
   benchmark workloads.

Every implementation phase ends with the same global audit, not only tests for
the files changed in that phase.

## 12. P0 whole-architecture review

P0 changed no production Rust behavior. Its purpose was to make the design and
evidence independently falsifiable before the old encoding is deleted.

| Review surface | P0 result | Correction / evidence |
|---|---|---|
| authority and location | PASS | `architecture-contract.json` admits exactly `TxPool` and `PrePoolKernel`, exactly seven pre-pool states and an explicit forbidden-state set |
| transition and failure semantics | PASS after model correction | the independent model exercises all five Plan outcomes; capacity rejection no longer consumes a version, and an over-budget worker completion terminalizes with typed Backpressure instead of leaving a leased owner |
| ABA and identity | PASS | stale completion after different-witness replacement is a no-op; full hash, `wtx_hash`, short-ID role and one non-reused `u128` version are machine-readable |
| derived views and progress | PASS at oracle level | the model independently rebuilds resolve/verify queues, Wait/reverse-dependency views, Ready/global-input order and exact charge after every one of 8,000 generated commands |
| historical attack coverage | PASS | CI parses ordered ledger IDs 1-152 and composes every I1-I12 row through a non-empty F1-F8, T1-T13 and TP behavior bridge |
| integration impact completeness | PASS | 149 managed specs replace the earlier 10-anchor subset; validator proves every registered `specs/tx_pool` type, every runtime-named spec and every registered source containing a direct tx-pool boundary is included and present in `ckb-test --list-specs` |
| resource/performance topology | unchanged | production remains 60 files / 24,236 physical lines; test and benchmark source are reported separately; current direct guard calibration is corrected to 34 sites in 11 files; no benchmark ran |
| lock/effect/reorg/template/persistence | unchanged and still open for implementation | the contract freezes the target order and effect/authority regions; C1 regressions remain mandatory until their P1-P4 target replacements exist |
| extensibility/reviewability | PASS | architecture, audit, implementation plan, guide, ledger, manifest, inventory and integration universe are linked; generated evidence rejects drift rather than duplicating handwritten lists |

No section-11 stop condition fired. P1 is authorized only as a vertical
replacement: it may not wrap the coordinator, retain ConflictCache ownership,
or introduce an adapter state between the old and target models.
