# Tx-Pool Architecture: Two-Authority Plan/Apply Kernel

Status: implementation authority for the pipeline refactor at checkpoint
`6d0577ad4`. Execution status is tracked in
[`IMPLEMENTATION_PLAN.md`](IMPLEMENTATION_PLAN.md), independent conclusions in
[`ARCHITECTURE_AUDIT.md`](ARCHITECTURE_AUDIT.md), and executable review evidence
in [`REVIEW_GUIDE.md`](REVIEW_GUIDE.md).

## 1. Decision

The final architecture has two, and only two, executable transaction owners:

1. `TxPool` owns accepted transactions and consensus-facing pool status.
2. `PrePoolKernel` owns retained transactions that have not been accepted.

Every pre-pool transaction is one primary entry in exactly one of six
locations. Queues, wait edges, deadlines, conflict indexes and counters are
derived projections containing identity or accounting only. Accepted mutation
is a read-only `PoolMutationPlan` followed by total `apply_mutation`. Stable
observable effects are appended in the same innermost critical section as
state Apply.

This is deliberately not a universal actor, an undo/rollback engine, a
panic-restart system, or a generic workflow framework. It is the smallest model
found that closes the concrete `develop` ownership, RBF atomicity, liveness and
resource-bound failures without serializing read-heavy accepted-pool work.

## 2. Goals and non-goals

The architecture must:

- make retained payload ownership explicit and queryable;
- make stale asynchronous completions harmless through one non-reused version;
- reject all ordinary policy/capacity failures before accepted Apply;
- preserve exact RBF, dependency, reorg, proposal and template liveness;
- bound bytes, entries, active work and graph fan-out before retention/work;
- publish callbacks, relay decisions and diagnostics without state/effect gaps;
- isolate untrusted computation and endpoints using Rust-native unwind
  boundaries while failing fast on impossible authority corruption;
- retain Local RPC's direct synchronous validation path by design;
- preserve or improve pipeline throughput, subject to final checkpoint A/B.

It does not attempt to:

- make the process survive OOM, abort, FFI corruption or arbitrary memory
  corruption;
- treat an invariant panic as a legal transaction result;
- provide durable mempool persistence across crashes;
- make callbacks or network endpoints exactly-once;
- replace consensus verification, the accepted `PoolMap`, or block-template
  policy with a general scheduler.

## 3. Why `develop` requires structural change

The refactor is justified by failure families, not by a preference for new
types.

| Family | `develop` defect | Required structural boundary |
|---|---|---|
| F1 ownership/ABA | queue pop, active work, orphan retention, clear and re-admit infer location from several structures; stale work can erase or duplicate ownership | one full-hash primary entry and one non-reused version |
| F2 accepted atomicity | RBF/capacity paths can remove accepted transactions before every replacement condition is known | immutable accepted Plan and total Apply |
| F3 stable effects | mutation and relay/callback publication can be separated by saturation or endpoint failure | bounded journal append coupled to Apply |
| F4 resource hostility | queue, orphan, conflict and graph metadata have separate or incomplete accounting | one retained-entry charge plus explicit graph/active bounds |
| F5 dependency liveness | missing/conflicting work can be parked in disconnected mechanisms and miss a wake | one `Wait` owner with exact, level-triggered reverse keys |
| F6 conflict recovery | speculative victim ownership and failed-RBF restoration require ordering-sensitive undo | no speculative removal; conflict history is optional metadata in the same kernel |
| F7 chain/admin ordering | reorg, clear, save and template updates can observe different ownership generations | `TxPool -> PrePoolKernel -> EffectJournal` chain transition plus epoch/reset authority |
| F8 identity/status | raw hash, witness hash, proposal short ID and RPC status can be used as if interchangeable | explicit identity domains and collision-aware projections |

Local hardening of each legacy queue cannot prove the partition invariant: the
bug is the inferred handoff between structures. Conversely, moving accepted
`PoolMap` into one global actor would simplify proof but regress concurrent
read/template/RPC performance and create a single mailbox bottleneck. Two
authorities are the minimum separation that preserves both proof and useful
concurrency.

## 4. Ownership and state

### 4.1 Partition

For each full transaction hash `h`:

```text
owners(h) = accepted(h) + prepool(h)
owners(h) is 0 or 1
```

`accepted(h)` is membership in `TxPool`. `prepool(h)` is membership in the
kernel primary map. The commit, clear, remove and reorg paths hold the accepted
pool guard while performing the matching kernel handoff, so no observer can
see an ownership gap between those two authorities.

### 4.2 Six pre-pool locations

```text
ResolveQueued
ResolveLeased
Wait(Missing | Conflict)
VerifyQueued
VerifyLeased
Ready
```

`Missing` and `Conflict` are reasons inside the one unavailable-work location;
they do not own payloads independently. Recovery is a trusted source, not a
state. There is no persistent `Committing`, `Invalidated`, `RaceLost`,
`RecoveryRetained`, victim hold, undo state or conflict-owned payload.

The entry owns:

- compact raw transaction payload;
- source attribution;
- exactly one typed state payload;
- full-hash version and arrival clocks;
- expiry and conservative retained-byte charge;
- canonical dependency keys.

Derived projections own no transaction payload:

- fair resolve/verify queues;
- proposal-short-ID index;
- peer, parent, waiter, deadline, ready and ready-by-input indexes;
- total/remote/per-peer/conflict residency;
- total/per-owner active work;
- bounded availability epochs and dirty wake keys.

### 4.3 Sources are policy, not location

`PrePoolSource` is `Remote(peer)`, `Proposal`, or `Recovery`.

- Remote retains immutable ingress and blame attribution, declared cycles,
  remote/per-peer charge and expiry.
- Proposal is trusted and may promote the same-witness Remote entry or replace
  its payload with a trusted witness variant.
- Recovery is trusted detached-chain input and enters the ordinary resolve
  lifecycle parent-first.
- Local never enters the pre-pool. It validates synchronously, commits under
  the same accepted Plan/Apply transaction, and settles any matching retained
  owner. This is an intentional API behavior, not an optimization accident.

## 5. Identity domains

| Identity | Only valid role |
|---|---|
| full transaction hash | primary ownership, accepted membership, lease owner |
| witness hash (`wtx_hash`) | `TxVerificationCacheKey`; verification results cannot alias witness variants |
| proposal short ID | collision-aware index and consensus proposal protocol only |
| entry version (`u128`) | process-global, non-reused ABA token for leases/tickets |
| pipeline epoch | administrative clear/reorg invalidation, not per-entry identity |

A short-ID collision is backpressure/rejection, never proof of duplicate
ownership. Cache accesses construct `TxVerificationCacheKey::from_transaction`.
A stale completion must match full hash, exact version and expected location
before it can mutate the primary.

## 6. State transitions

The public transition family is closed:

| From | Command/outcome | To |
|---|---|---|
| Nowhere | admit Remote/Proposal/Recovery | ResolveQueued |
| retained location | trusted same-hash promotion | same semantic phase or ResolveQueued for a new trusted witness |
| ResolveQueued | checkout exact queue head | ResolveLeased(new version) |
| ResolveLeased | resolved | VerifyQueued(new version) |
| ResolveLeased | exact dependency unavailable | Wait(Missing/Conflict) |
| Wait | all observed keys available | ResolveQueued |
| VerifyQueued | checkout exact queue head | VerifyLeased(new version) |
| VerifyLeased | verified and fee-gated | Ready(new version) |
| VerifyLeased | snapshot/parent stale | ResolveQueued or Wait |
| Ready | selected commit handoff | accepted or bounded Conflict wait/history |
| any retained location | reject/remove/expiry/peer removal/clear | Nowhere |
| any retained generation | chain/admin generation reset | sealed retired generation, then Nowhere/new recovery entries |

Every single-entry transition validates its next entry, exact usage delta,
active-work delta and index constraints before detaching the old projections.
Bounded multi-entry transitions compile a `CohortPlan` containing final
primaries and exact final counters, then move those primaries during total
Apply. There is no rollback path.

Legal outcomes are typed:

- transaction/policy rejection;
- bounded capacity/backpressure;
- stale lease or location race;
- duplicate/idempotent arrival;
- Apply.

Primary/index/accounting contradictions are assertions, not another outcome.

## 7. Scheduling and progress

### 7.1 Fair work queues

Resolve and verify queues contain `WorkKey` identities only. Each source owner
has a queue with a fair turn; runnable heads are derived from global and
per-owner active limits. A large-cycle verify item cannot hide eligible
small-cycle work from a constrained worker.

### 7.2 Ready order

Ready selection uses one total `ReadyKey`, strongest last:

1. source priority: Remote < Proposal < Recovery;
2. exact fee rate by `u128` cross multiplication;
3. absolute fee;
4. earlier arrival;
5. smaller full hash;
6. entry version;
7. transaction size as the final total-order tie breaker.

A `CommitTicket` proves the selected entry's hash, version and rank. A later,
stronger Ready entry does not invalidate an already selected exact ticket; the
single commit driver settles it, then selects the new head. This avoids a legal
arrival race becoming an invariant failure or livelock.

### 7.3 Wait is level-triggered

`Wait` records the exact dependency keys and their observed availability
epochs. Availability changes dirty a bounded key; maintenance drains bounded
slices. A missed notification is harmless because the level remains visible.
Parent loss demotes resolved/verified consumers using the same exact keys.
Final accepted-pool validation remains authoritative even if background
demotion has not run yet.

## 8. Resource proof

Admission or transition planning accounts conservatively before retention:

- total pre-pool entries and bytes;
- Remote entries and bytes;
- per-peer entries and bytes;
- optional conflict-history entries and bytes;
- total active work and per-peer active work;
- dependencies per entry and dependents per parent;
- Ready inputs and candidates per input;
- accepted pool serialized bytes, retained bytes, cycles and mutation closure;
- effect batches/bytes by trust region;
- candidate uncle count and per-height count.

The charge is held continuously across queued, leased, waiting and Ready
locations. A worker borrows an `Arc`; it does not become a new owner or refund
residency. Optional conflict history degrades to a terminal result when its
partition is full. Graph operations stop at explicit product/fan-out bounds
before mutating.

`NotifyTxs(Vec<TransactionView>)` is a trusted controller boundary. Each item
still passes validation and admission accounting, but the vector itself is not
currently proven by this module to have an upstream length bound; this remains
a documented integration-boundary risk rather than a hidden invariant claim.

## 9. Accepted Plan/Apply

Ordinary admission, RBF and capacity eviction use the same transaction:

1. Under the accepted pool write guard, calculate RBF conflict closure,
   ancestry, status, candidate parents, capacity evictions and post-state
   totals without mutation.
2. Return every ordinary `Reject` from Plan.
3. While accepted membership is unchanged, settle matching/preempted
   `PrePoolKernel` ownership and parent availability.
4. In the innermost effect-journal critical section, execute total accepted
   Apply and append the stable effect batch.
5. Release locks, then run callbacks/network/database endpoints.

`PoolMutationPlan` contains the candidate, final status, exact removals and
post totals. `apply_mutation` performs only checked moves and assertions that
the planned generation remains present. It cannot discover fee, RBF, ancestry,
identity or capacity policy after removing a victim.

This removes the need for nested undo. Rollback is the wrong abstraction here:
it duplicates ownership, index and accounting semantics, and its own failure
becomes a second correctness problem. Read-only Plan plus total Apply proves
the original state is unchanged on every legal failure.

## 10. Cross-authority locking

The nesting order is:

```text
optional serial/work permit
  -> EffectJournal capacity hint (released before state locks)
  -> TxPool read/write
    -> PrePoolKernel mutex
      -> EffectJournal mutex for capacity + total Apply + append
```

No await, callback, network/database endpoint or retired-generation drop occurs
while the accepted pool/kernel/journal nesting is held. Kernel-only worker
transitions use only the shorter kernel→journal suffix. Code must not acquire
`TxPool` from an effect callback or while already holding the kernel.

The journal capacity wait is a hint, not a reservation. A racing append can
make the innermost attempt return `Full`; the caller releases state locks,
waits/replans and retries. No authoritative mutation is replayed.

## 11. Stable effects

The effect journal has statically partitioned trust ceilings:

- Remote can consume only the Remote ceiling;
- Trusted may use ordinary capacity plus trusted headroom;
- Critical chain/admin work has independent headroom.

For a precomputed exact batch, `try_apply` checks capacity, executes total state
Apply and appends one sequence while holding the journal mutex. For effects
materialized during accepted Apply, `try_apply_bounded` checks a proven static
upper bound first and appends the exact batch afterward. A violated bound does
not overcharge the FIFO; observers converge through a prebuilt constant-size
`GenerationReset` register.

The publisher drains FIFO order. Full relayer capacity retains the active head;
individual relay detail may coalesce to `GenerationReset`, which stays pending
until accepted. Callback execution is isolated on a bounded endpoint thread
with timeout/circuit behavior; endpoint panic cannot unwind state Apply.

Exactly-once delivery is not claimed. The invariant is: after state Apply, a
bounded stable record exists until detailed publication succeeds or a newer
authoritative generation reset subsumes it.

## 12. Dependencies and conflicts

Three relations are intentionally distinct:

1. causal producers: inputs and expanded dep-group producers;
2. conditional readers: cell/header dep ordering constraints;
3. availability keys: exact causes that keep a pre-pool entry in `Wait`.

Accepted `PoolMap` normalizes out-point memberships into set semantics so the
same input/dep cannot publish or remove one logical edge twice. RBF conflict
closure is bounded by the complete input × candidates product before traversal.
The accepted Plan builds a virtual post-removal overlay for ancestry and
availability; total Apply then moves exactly that closure.

Failed RBF does not remove and restore victims. A verified losing transaction
may remain as bounded `Wait(Conflict)` history only after it has passed the
higher fee gates. When its keys become available it returns through ordinary
resolution and final RBF validation. Optional-history saturation terminalizes
the loser rather than blocking the winning commit.

## 13. Reorg, clear and persistence

### 13.1 Reorg

The chain-authoritative phase holds the accepted write guard and kernel mutex:

1. reconcile attached/detached blocks and accepted statuses once;
2. collect detached transactions plus the accepted descendant closure of
   detached producers;
3. order one combined recovery set parent-first;
4. compile/apply one bounded trusted kernel recovery plan;
5. if that complete set cannot be represented within the frozen bound, reset
   the ephemeral accepted/pre-pool generation rather than retain an invalid
   parentless suffix;
6. journal the immediate block-assembler reset and optimistic generations.

The recovery payload is an ordinary `PrePoolSource::Recovery` entry in one of
the six locations. No handler-local payload, cross-await `recovery_lock`,
`RecoveryRetained` location or replay retry owner exists. Duplicate attached
raw hashes suppress detached witness variants; cache lookup uses witness hash.

### 13.2 Gap and uncle liveness

Reorg status reconciliation reevaluates Gap entries against the new proposal
window, including Gap→Pending demotion. Block template proposal packaging also
filters candidate uncles whose proposal IDs conflict with proposals that must
be repackaged. A detached block cannot remain an eligible uncle and suppress
the only proposal path of its recovered transactions for an epoch.

Block assembler authority is intentionally asymmetric:

- Reset and `update_full` serialize on the complete-template boundary;
- `update_full` has highest priority;
- proposal/transaction updates are optimistic, level-triggered deltas and may
  be skipped transiently;
- a failed reset remains pending and blocks ordinary deltas until a matching
  full rebuild succeeds;
- zero update interval applies deltas eagerly, retries resets periodically and
  suppresses external miner notification only as configured.

### 13.3 Clear and save

Clear advances the administrative epoch, takes the accepted guard, swaps the
entire pre-pool generation in O(1), installs a generation reset, releases the
guard and only then destroys retired payloads. Stale workers cannot mutate the
new generation because versions never repeat.

Explicit save serializes accepted plus retained recovery-relevant transactions
in dependency order. Graceful shutdown persists only after supervised state
workers and the effect publisher finish normally. An invariant failure marks
persistence ineligible so a corrupt derived state is not written as truth.

## 14. Rust-native failure model

The design does not pursue “panic-free Rust” or emulate Erlang supervision.
It separates expected results from programming contradictions:

| Class | Rust encoding | Boundary |
|---|---|---|
| malformed/policy | `Reject` / transaction `PrePoolError` | reject/ban according to policy |
| capacity/backpressure | typed error | no mutation; wait/retry or bounded degradation |
| stale/duplicate race | typed stale/duplicate | discard, preserve current owner or retry level |
| shutdown/config/resource | typed external result or startup failure | controlled stop/fail startup |
| resolver/verifier panic | `catch_unwind` around the borrowed job | terminalize/quarantine that job; worker continues where safe |
| callback/endpoint panic or hang | endpoint boundary, timeout/circuit | state remains committed; publisher progresses/coalesces |
| primary/index/accounting contradiction | assertion/`expect` inside authority | fail fast, cancel service, skip persistence |

Legal hostile input must not reach the last row. Assertions are justified only
where Plan or the primary entry construction statically established the fact.
Adding a recover-and-repair path for an impossible Apply contradiction would
expand the state machine, risk persisting corruption and turn every assertion
into a poorly specified operational branch.

The remaining operational consequence is intentional but explicit: a genuine
internal invariant defect stops the tx-pool service. The defense against a
repeatable peer DoS is to ensure all peer-selectable parsing, policy, capacity,
stale and endpoint outcomes are typed or isolated before the assertion
boundary—not to restart an unknown authority generation.

## 15. Block-template authority

Accepted status is consensus-facing and remains `Pending`, `Gap` or `Proposed`.
RPC compatibility may project Gap as pending, so review/tests must inspect
internal detail when liveness depends on the distinction. Proposal selection
iterates true Pending; transaction selection commits true Proposed.

Template state has two forms:

- a full authoritative snapshot generation, updated by Reset/`update_full`;
- coalesced proposal/transaction dirty generations applied optimistically.

Candidate uncles are bounded at production limits (128 total, 10 per height)
in both production and tests. An uncle is removed/rejected when it is on the
main chain, already embedded, epoch/target-invalid, structurally invalid, or
conflicts with proposal liveness. Test builds do not change these limits.

## 16. Performance model

Correctness structure is also the intended performance structure:

- one short synchronous kernel mutex section per retained transition;
- identity-only fair queues and reverse indexes;
- `Arc` borrowing for raw/resolved/verified payloads;
- stack-sized plans for single-entry transitions;
- heap-backed cohort planning only for bounded multi-entry changes;
- move-only Apply; no cloned old-entry snapshot/undo journal;
- one serialized commit driver instead of competing commit owners;
- accepted reads remain behind the existing `RwLock`, not a global actor;
- no I/O or payload destruction under authority locks;
- block-template/effect notifications coalesce level-triggered work.

Performance acceptance is empirical, not inferred from line count. The final
gate compares clean checkpoints with controlled warmup/cache conditions and
measures throughput, tail latency, RSS/allocation, lock hold time, worker
utilization, commit, reorg and template latency. A material regression blocks
production even if correctness tests pass.

## 17. Extensibility rules

A new stage or policy is acceptable only if it answers these questions:

1. Does it require retaining a payload, or can it be derived metadata/work?
2. Which of the two authorities owns it at every instant?
3. Can it be encoded inside an existing state payload/reason/source rather
   than a seventh location?
4. What exact byte, entry, active and graph bounds apply before retention?
5. What versioned command consumes it, and what are all legal outcomes?
6. Does it alter accepted membership? If so, where is immutable Plan and total
   Apply?
7. Which stable effects must be appended with Apply?
8. What level-triggered condition guarantees progress after a lost wake?
9. Which review behavior and unit/process regression prove the boundary?
10. What benchmark demonstrates no material hot-path regression?

A proposal that adds a payload-owning side cache, rollback journal, broad
retry, reverse lock order, unbounded vector/graph, or test-only production
behavior is rejected at design review.

## 18. Why this is the preferred architecture

### Versus hardening `develop`

Per-queue locks/checks cannot make a transaction's location single-valued
across pop, await, cancellation, conflict parking and clear. Fixes remain
ordering-dependent and every new queue adds another handoff proof. The kernel
removes the root inferred-location premise.

### Versus a universal tx-pool actor

One actor could serialize all ownership but would route accepted reads,
template queries, RPC inspection and graph computation through a mailbox. It
reduces concurrency, raises tail latency/backpressure coupling and makes large
messages part of the trusted scheduling surface. The two-authority design
serializes only mutation boundaries that require atomicity.

### Versus nested undo/transactions

Undo snapshots duplicate state and require a second correct implementation of
indexes, budgets, victims and wakeups. Apply failure then needs rollback failure
semantics. Plan/Apply preserves the original state until all ordinary failure
conditions are exhausted and has one mutation implementation.

### Versus persistent commit/conflict/recovery states

`Committing`, `RaceLost`, victim holds and `RecoveryRetained` encode protocol
execution as durable ownership. They enlarge the partition, persistence,
expiry, clear and ABA matrices. A single commit driver, bounded `Wait` reason
and ordinary Recovery source provide the required semantics without those
locations.

### Versus invariant-repair/restart

Repairing an authority after a contradiction requires trusting projections
already proven inconsistent and choosing which externally visible effects to
replay. Rust unwind isolation is appropriate at untrusted computation and
endpoint boundaries; assertions plus fail-stop are appropriate for impossible
primary/projection contradictions. This is simpler and more honest, provided
hostile legal outcomes are fully classified before Apply.

## 19. Proof obligations

The machine contract is [`architecture-contract.json`](architecture-contract.json).
The human proof obligations are:

- T1 Partition: accepted and pre-pool ownership are disjoint and unique.
- T2 Lease: only exact full-hash/version/location work mutates a primary.
- T3 Budget: retained ownership iff it is charged; all fan-out/work is bounded.
- T4 WaitExactness: Wait owns exact observed dependency causes.
- T5 ReadyExactness: Ready ranks/inputs derive from its primary payload.
- T6 AtomicAcceptance: every ordinary failure precedes accepted Apply.
- T7 StableEffects: successful Apply leaves a bounded publishable record.
- T8 BoundedHostility: peer input cannot cause unbounded residency/CPU/fan-out.
- T9 ChainSerialization: reorg/clear/save observe one authority order.
- T10 CriticalSchedulability: Remote saturation cannot consume chain headroom.
- T11 LevelTriggeredProgress: lost wake edges do not strand executable work.
- T12 AcceptedStatusExactness: Pending/Gap/Proposed transitions match snapshot.
- T13 TemplateAuthority: reset/full/deltas cannot publish an old-parent or
  proposal-stranding template.

The 152 historical findings remain mapped to I1–I12 in
[`security-regression-ledger.md`](security-regression-ledger.md). The mapping is
evidence history, not permission to retain obsolete mechanisms.

## 20. Residual risks and release conditions

- R1: OOM/allocator abort and process-level corruption are outside in-process
  recovery.
- R2: a genuine internal invariant defect stops tx-pool and skips persistence;
  this is safe but operationally visible.
- R3: callbacks/network delivery are at-least-bounded, not exactly-once.
- R4: explicit pool persistence is not a crash-durable transaction log.
- R5: trusted controller batch length needs an upstream bound audit.
- R6: final performance superiority is unproven until P7 checkpoint A/B.
- R7: complete process-level integration acceptance remains P6 work.

No release claim is valid until document validators, all `ckb-tx-pool`
`nextest`, the complete 149-spec integration impact universe and final
performance gates pass. Findings that are low-value, incompatible or unproven
must be recorded as residuals rather than hidden by another mechanism.
