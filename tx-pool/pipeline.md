# Tx-Pool Pipeline Architecture

This document describes the production architecture after the coordinator
cutover. It is a contract for correctness, security, performance, and future
changes; it is not a migration sketch. Historical findings and their current
regression anchors are tracked in
[`security-regression-ledger.md`](security-regression-ledger.md).

## 1. Design goals

The pipeline must satisfy all of these simultaneously:

1. A pre-pool transaction has one executable owner and one typed lifecycle
   state.
2. `TxPool` is the sole accepted-membership authority.
3. The existing `TxPool` write guard is the normal commit sequencer; RBF is
   recalculated there and never trusted from speculative scheduling.
4. State mutation and externally visible effects have one cancellation-safe
   linearization boundary.
5. Untrusted payloads, graph edges, tickets, active work, historical conflicts,
   and queued effects remain continuously bounded.
6. Parent/conflict progress is event-driven and bounded per maintenance slice.
7. Reorg, RPC visibility, block-template selection, and persistence observe
   compatible state.
8. The refactor must not reduce throughput. Correctness gates precede the final
   controlled A/B performance gate; benchmark execution requires explicit
   instruction.

## 2. Authorities and ownership

There are three deliberately different stores:

| Store | Meaning | May execute? | Authority ends when |
|---|---|---:|---|
| `PipelineCoordinator` | Every remote/proposal transaction before acceptance | Yes | Commit handoff, terminal outcome, administrative removal, clear, or expiry |
| `TxPool` / `PoolMap` | Accepted pending/gap/proposed membership and dependency graph | Yes | Chain reconciliation, RBF/limit eviction, expiry, remove, or clear |
| `ConflictCache` | Bounded historical copy of a transaction rejected behind an accepted input owner | No | Atomic admission to the coordinator, eviction, explicit removal, or clear |

No queue, worker, deferred channel, RPC cache, or block template is an
additional lifecycle owner. Queue/deadline/dependency/conflict structures in
the coordinator contain IDs and versioned tickets only. The verification cache
is an optimization and may lose updates without affecting ownership.

Scheduling source and ingress attribution are deliberately separate. Source
may be promoted from Remote to Local/Proposal, changing priority and resource
ownership; the raw payload retains the immutable first peer until its relay
filter receives one success or terminal settlement.

### Cross-authority order

An operation that needs accepted and pre-pool state takes the async `TxPool`
guard first, then the coordinator's short-held synchronous mutex. Coordinator
code never acquires `TxPool`, and its mutex is never held across `.await`.
Combined queries use the same order, so they cannot observe a transaction in
neither owner or in both owners during a handoff.

`recovery_lock` is outside `TxPool` and is used only to exclude persistence or
another chain-wide recovery from an incomplete retained-transaction replay.
Normal remote commits do not take it and reserve effect capacity before
`TxPool`. Reorg, clear and persistence instead share one chain-wide order:
`recovery_lock -> effect credit (when required) -> TxPool`. No caller may hold
effect credit while waiting for `recovery_lock`, because detached replay can
reserve ordinary submit/reject effects while retaining that lock. Mutating
callback re-entry fails fast, so the publisher cannot add the reverse edge.

## 3. Submission paths

### Remote and proposal transactions

Remote/proposal input enters `PipelineCoordinator` with a full-hash identity,
short ID, source, epoch, raw payload, dependency set, expiry, and complete
residency charge. Its typed state moves through:

```text
RawQueued(PreCheck) -> RawActive(PreCheck)
  -> RawQueued(Resolve) -> RawActive(Resolve)
  -> WaitingParents | VerifyQueued
  -> VerifyActive
  -> Verified
  -> Committing
  -> TxPool or terminal record
```

Each worker receives a lease containing incarnation/revision evidence. A stale
finish cannot modify a newer owner. Checkout and completion preflight sequence
capacity before consuming the only live ticket. Source promotion is a typed
transition, not a second entry: it atomically releases remote peer residency,
cancels the remote expiry and its charge, retickets queued work, and schedules
the derived conflict-rank/ticket delta immediately when a verified candidate's
priority changes.

`RawStage`, payload phase, conflict metadata, location, and invalidation are
closed `EntryState` variants. Invalid combinations such as “verified payload
without verified state” or “invalidated candidate still owning input indexes”
cannot be created through the public transition API.

### Local and reorg-retained transactions

Local RPC submission is intentionally synchronous: resolve, verify, submit,
and return the definitive result in the caller path. Reorg-retained replay uses
the same direct core. These paths do not become asynchronous coordinator work
before verification. On successful pool insertion they invalidate any older
coordinator copy under `TxPool -> coordinator`, preserving single ownership.

The final epoch check occurs inside the pool write transaction, so clear is a
linearizable cancellation boundary.

## 4. Dependencies, conflicts, and scheduling

### Missing parents

The coordinator indexes child IDs by full parent hash. Raw admission records
the dependencies visible in the transaction, then successful resolution and
missing-parent/verify-demotion transitions atomically extend that same graph
with dep-group members discovered by the resolver. The extension, reverse
index, metadata charge, payload demotion or verify enqueue, and remote
`UnknownParents` effect are one undo-protected transition. Policy, fanout, and
capacity failures are typed transaction outcomes; ordinary remote dep-group
input cannot reach fail-stop.
Trusted Local/Proposal input with a parent that is neither accepted nor
coordinator-owned fails terminally; remote input waits with an expiry and
parent-request protocol.

Parent commit wakes direct children in the same coordinator transition. A
definitive parent exit invalidates direct children immediately; transitive
cleanup drains through the maintenance queue in fixed slices. Cycles,
dependency depth, per-parent fanout, metadata edges, and cascade victims are
bounded.

### Verified conflicts

Only verified candidates can participate in coordinator conflict preference.
Unverified declared cycles cannot preempt or censor verified work. Candidate
comparison uses trusted source priority and verified size-based fee score;
exact ties retain the earlier owner. Multi-input acquisition and release are
all-or-none, and `Committing` ownership is frozen.

This ordering is provisional scheduling only. Under the `TxPool` write guard,
commit recomputes the current conflict closure, both RBF fee gates, ancestor
constraints, pool limits, and final status against the current snapshot.

### RBF pool transaction

One `PoolCommitJournal` records every physical removal and its cause:
replacement, ancestry escape, or size limit. A successful insertion finalizes
the coordinator lease before the pool guard opens. If insertion or coordinator
finalization fails, the exact removed entries are restored parent-first with
their original status inside the same guard. No callback or competing
candidate can observe an evict-then-restore gap.

Block-assembler dirty states include both the inserted status and every removed
status. A Pending replacement of a Proposed victim therefore refreshes both
template components.

## 5. Historical conflict recovery

`ConflictCache` is a bounded, non-executable store inside `TxPool`, keyed by
the complete raw transaction hash and indexed by compact recovery outpoints:
inputs, direct cell deps, and every dep-group member known from an accepted
victim. Proposal short IDs are never identity. Duplicate history monotonically
unions wake metadata without weakening the source or witness. Count and
resident-byte caps cover the payload and every reverse-index key.
Generation-tagged FIFO tickets and bounded-ratio compaction prevent
remove/reinsert churn from growing stale metadata or letting an old ticket act
on a new incarnation.

Every pool mutation that changes outpoint availability registers the delta
while holding the `TxPool` write guard: removals publish truly released inputs,
accepted transactions publish their outputs, and attached-chain transactions
publish their outputs. Physical removal is first projected through the active
snapshot: removing an overlay whose transaction is already committed does not
advertise its chain-consumed inputs as free. A later maintenance slice probes
at most 32 cache candidates through a stable per-outpoint cursor; no removal
path scans an attacker-controlled 10k-candidate fanout while holding the pool
guard. Eligible candidates then follow the ownership handoff:

1. reserve terminal-effect capacity;
2. acquire `TxPool`;
3. pop one generation-valid cache candidate;
4. recheck accepted-input conflicts and liveness of every recovery outpoint;
5. admit it to the coordinator while the pool guard remains held;
6. remove the cache copy only after coordinator admission succeeds.

Thus the cache is sole owner before the handoff and the coordinator is sole
owner after it. A release-event identity prevents RBF victims from being
immediately re-admitted by the same mutation that archived them. `Full`
reschedules the same cache generation and retries on a coarse tick without a
hot loop. A newly arrived accepted blocker leaves the candidate cache-owned
and unscheduled until that blocker later frees the input.

There is no deferred recovery channel and no publication barrier. The only
bounded asynchronous auxiliary channel carries best-effort verification-cache
updates; executable transactions never pass through it. The verification
cache itself is keyed by `TxVerificationCacheKey`, whose only constructor takes
a `TransactionView` and derives its witness hash. Raw transaction hashes cannot
compile as cache keys, including in detached replay and block verification.

## 6. Capacity and attack-cost closure

Coordinator residency charges payload plus conservative entry, dependency,
queue-ticket, deadline, and conflict-edge metadata. Limits include:

- global count and bytes;
- per-peer count and bytes;
- global and per-peer active worker slots;
- dependencies and dependents per parent;
- conflict inputs, candidates per input, and global conflict edges;
- dependency ancestors and capacity victims per transition.

Capacity reconciliation never evicts `Committing` work or an incoming
transaction's dependency ancestors. Peer-local impossibility fails before
unrelated global eviction. Every victim produces a terminal record and the
whole transition is undo-protected.

Accepted-pool ancestry uses the verified resolved dependency graph, including
expanded dep-group members. The cell-ref ancestor escape hatch distinguishes
required output producers from ordering-only cell references, plans an
immutable descendant closure, and mutates only after the exact plan fits the
shared 100-entry displacement bound. It never scans the complete eviction
index and never evicts a required producer. Late-parent linking uses one
complete descendant closure for both aggregate weights and reverse ancestor
updates, so CPFP/eviction keys cannot diverge on out-of-order replay.

Each stage queue is a two-level priority index: per-owner small/large heaps
publish one generation-tagged head into global any/small heaps. A peer at its
active-work cap therefore costs one skipped owner head, not a scan through all
of that peer's transactions; ABA-stale publications and tombstones are rejected
and compacted to bounded ratios. Multi-entry transitions reserve owner-local
heap credit up front and discard unused credit before releasing the runtime
mutex.

Async readiness is a derived scheduling projection, never another lifecycle
owner. After every coordinator transition, while the same mutex still exposes
one state version, the runtime evaluates the exact checkout predicate for five
worker classes: PreCheck, Resolve, VerifyAny, VerifySmall and Commit. It
publishes notifications only after unlock and only for classes with immediately
executable work. Queue non-emptiness alone is not readiness: active-work caps,
an in-flight commit, or a large-cycle item seen by a small-only verifier can
make a live queue ineligible. Consequently a failed checkout cannot wake itself,
but releasing an active slot re-evaluates and re-arms every newly executable
class. Verify capabilities have independent permits, and a replacement worker
re-derives readiness when it subscribes, so neither an incapable worker nor a
panicked generation can consume the sole wake needed by a capable successor.
The predicate probes only generation-tagged owner heads; capped owners are
bounded by the active-worker limit rather than queue population.

Global residency and verified-conflict reconciliation use weakest-first
`BTreeSet` indexes derived from the authoritative entries. The outermost undo
transaction publishes every affected key only after success; failure rebuilds
all derived indexes. The invariant audit independently reconstructs and
compares them. Operation-count regressions prove selection stops before a
100-entry stronger suffix. The remaining `min_by` searches are confined to
explicitly capped per-parent or per-input buckets, so no production victim or
scheduling path scans the whole live store.

The conflict cache independently compacts insertion and recovery tickets. The
effect outbox continuously charges reserved, queued, and active batches, so
moving a terminal payload out of a state owner does not create an uncharged
backlog.

Long-lived packed keys and views are materialized at ownership boundaries.
Coordinator dependencies, PoolMap indexes, conflict history, verification
cache keys, effects, liveness memo entries, recent-commit keys and candidate
uncles cannot retain a whole network message, transaction, or block backing
allocation through a 10/32-byte slice. The internal recently-banned peer race
fence is an expiring bounded LRU; the network service remains the authority
for the actual ban. Ordered reorg delivery has capacity one because deltas are
strictly serial and buffering block trees creates memory without concurrency.

## 7. Stable-state effects

Callbacks, relay results, peer bans, and reject notifications are immutable
`TxPoolEffect` records. Producers reserve bounded outbox capacity before state
mutation and commit the exact batch at the mutation boundary. The permit owns
its issuing queue; shrinking the conservative credit, allocating the mutation
sequence, and enqueueing are one outbox operation, with no externally
representable bound-but-not-queued state. A single publisher preserves FIFO
batch order, retries a full relayer without dropping the active head, and runs
endpoint code outside state locks.

Outbox capacity waiting follows a checked condition-variable protocol:
producers register their `Notify` waiter before checking the mutex-protected
budget, because `notify_waiters` does not retain a permit for a future waiter.
Capacity release and close may therefore wake all registered producers without
a check-to-sleep lost-wakeup window. The sole publisher uses a stored
single-consumer permit for ready/close and rechecks closed-plus-empty before
exiting.

There is one physical outbox but two producer contracts. A mutation-coupled
producer must reserve before the authoritative PoolMap/Coordinator transition
and journal inside that transition. A standalone producer is allowed only when
no lifecycle ownership is created, removed, or transferred—for example a
pre-admission rejection, an already-accepted duplicate acknowledgement, or a
local reject-history write. The current call sites satisfy that distinction,
but Rust types do not yet prevent a future mutation path from calling the
standalone helper; that reviewability debt is tracked as O12.

Relay success/reject and malformed-peer attribution use immutable ingress peer
identity, not the current scheduling source. Proposal promotion therefore
cannot leave a remote known-transaction filter stranded or erase responsibility
for the exact payload originally delivered by that peer.

Callback panics are contained inside callback dispatch and the FIFO cursor
advances. The outer publisher also quarantines an unexpected network-endpoint
panic after one attempt: the endpoint may already have performed an arbitrary
prefix, so retrying could duplicate effects and a permanent panic must not pin
all later batches. A callback may issue read-only controller queries.
Synchronous mutating controller re-entry fails fast because waiting for a
mutation from the publisher can form a cycle through outbox capacity or
`recovery_lock`. Each callback observes a completed local pool/coordinator
mutation; during multi-step detached replay, read-only RPC may observe the
current stable reorg slice while `recovery_lock` still excludes persistence.

Submit reservations are formula-derived: pool-removal callback payloads are
bounded by the prior pool plus one block-sized incoming transaction, while
coordinator settlement envelopes are bounded by the coordinator entry limit.
Reorg reservations are bounded by the prior pool after full-hash notification
coalescing. Pool-generated reject variants and commit-time ban diagnostics have
fixed checked display bounds. An unlisted or oversized post-mutation event is a
fail-stop invariant violation, not an under-reserved publication attempt.

Block-assembler notification uses a level-triggered dirty bit plus a bounded
wake channel. A full channel may coalesce a wake edge but cannot erase the
authoritative Pending/Proposed/Uncle/Reset work.

Pipeline invariant failures and accepted-pool failures have distinct monotonic
failure domains. A coordinator-only invariant stops the current service
generation and fails closed, but a clean quiescence may still persist the
unchanged accepted pool. A panic that crosses an authoritative `TxPool` /
coordinator/effect mutation boundary disables persistence for that generation.
Ordinary malformed, stale, duplicate, policy and capacity outcomes are typed
transaction results and never enter either failure domain. Automatic recovery
from an invariant panic is intentionally not attempted: without a complete
offending-input journal and rollback proof, continuing could expose poisoned
indexes or duplicate effects. The remaining availability tradeoff is tracked
explicitly in the security ledger.

## 8. Reorg and block-template consistency

The capacity-one reorg handler retains the FIFO head delta across panic/retry;
later deltas cannot overtake it. Its two phases retry independently: once the
pool/coordinator transition succeeds, failure of the derived block-assembler
refresh cannot replay that authoritative transition or resurrect an older
snapshot after a concurrent clear. After acquiring `recovery_lock` it reserves
critical outbox headroom that ordinary traffic cannot consume, then the pool
and coordinator apply attached commits, detached/unavailable parents, status
changes, conflict recovery scheduling, ingress-peer terminal results, and
effects in their defined order. Detached replay is topological and compares
attached identity by raw transaction hash. Clear uses the same resource order;
it cannot reserve ordinary credit while waiting for a reorg whose retained
submissions need that credit.

Overlapping detached proposal roots are removed as one union and replayed
parent-first exactly once per entry. This avoids repeated descendant mutation
and duplicate callbacks (`N` dependent detached IDs previously permitted
quadratic reprocessing). Reorg notifications are coalesced by full hash after
all expiry/limit/status work, then rebuilt from the final authoritative pool
entry; a transaction cannot publish an intermediate Pending event before its
final Proposed or terminal state.

Reorg status reconciliation explicitly demotes stale `Gap` entries when the
new proposal window no longer justifies them. Block-template proposal
selection also prevents optional detached-block uncles from suppressing the
only proposal path for recovered transactions. The regression mines a
parent/child/grandchild tree through normal `get_block_template`; it does not
inject a manual proposal block. The first post-startup reorg also performs one
semantic reconciliation against its fresh snapshot: already-committed
overlays, dead inputs/cell deps, and header deps no longer on the active main
chain are removed with their descendant closure.

Attached transaction outputs are also conflict-history wake edges. This
matters when an earlier input release was already consumed while another
required parent was absent: mining or attaching that parent must re-arm the
cache candidate rather than leave it externally invisible and permanently
owned by history.

Template updates preserve the original priority model. `reset_template` and
`update_full` serialize through `template_lock`, and a full rebuild performs
the unconditional highest-priority swap. Proposal and transaction updates are
optimistic version-CAS deltas; a skipped delta is safe because a successful
full swap reissues both authoritative dirty generations. Uncle updates also
take `template_lock`, specifically because a full rebuild carries forward the
uncle set it read. Reset generations are acknowledged conditionally, so an old
reorg refresh cannot erase a newer clear/reset.

Proposal byte accounting uses the consensus block-size basis. Proposal
selection is computed independently of optional candidate uncles, after which
conflicting uncle subtrees are filtered. Candidate-uncle insertion validates
before evicting an existing candidate, compacts accepted views, and only an
accepted insertion marks template work dirty. Selection is capacity bounded;
stale/main-chain/embedded candidates are removed, and optional uncle content
cannot strand a valid pool transaction.

## 9. Administration, shutdown, and persistence

Remove, peer ban, clear, and reorg call coordinator terminal/demotion APIs
rather than enumerating queue implementations. Full hashes disambiguate short
IDs at authority boundaries. `clear_pipeline` advances the epoch and clears the
coordinator plus every pending conflict-cache transfer ticket, while preserving
non-executable conflict history. A second epoch check under the pool lock keeps
old maintenance from resurrecting coordinator work. `clear_pool` also replaces
accepted state and journals a template reset.

Graceful shutdown proceeds in causal order:

1. stop controller dispatch and drain in-flight handlers;
2. quiesce state workers, maintenance, reorg, assembler, and cache worker;
3. if any worker timed out or an authoritative/effect boundary failed, abort
   remaining work and skip persistence; a coordinator-only failure may retain
   accepted-pool persistence after clean quiescence;
4. close and drain the effect outbox;
5. persist only after all state/effect boundaries completed.

This prevents a half-recovered or effect-incomplete pool from being saved as a
clean shutdown image.

Persistence writes accepted entries in the authoritative `PoolMap` dependency
order, including expanded dep-group members. It deliberately does not run a
second raw-transaction-only topological sort, because that incomplete graph
can move a child ahead of an expanded parent whose own raw parent is not ready.

## 10. Invariant and regression gates

The coordinator invariant audit reconstructs full-hash, short-ID, peer,
dependency, conflict, queue, deadline, active-work, residency, and victim
priority indexes from typed entries and checks physical live-ticket/head
equality. Transition fault injection verifies undo restoration across
multi-entry changes. A separate `PoolMap` audit reconstructs status indexes,
links, outpoint/header indexes, exact serialized/resident totals and
ancestor/descendant weights from accepted entries; cached counters repair only
from that authority and never saturate silently.

A stage is complete only after both reviews pass:

- incremental review: the stage's code and focused regressions;
- whole-architecture review: ownership, causal exits, lock/wait graph,
  state→effect atomicity, capacity/attack cost, RPC/template/persistence
  consistency, algorithmic complexity, readability, and new regressions.

Required correctness commands near a checkpoint:

```bash
cargo nextest run -p ckb-tx-pool --features internal
cargo nextest run -p ckb-verification-contextual
cargo fmt --all -- --check
git diff --check
```

The serial `cargo test ... --test-threads=1` suite is also useful for shared-
process ordering, but nextest is the acceptance gate for process isolation and
parallel scheduling. The final normal-mining reorg trio runs against freshly
built `ckb` and `ckb-test` binaries, never stale artifacts.

Integration acceptance includes the normal-mining reorg dependent-tree test,
RBF success/failure families, clear/remove races, callback re-entry, shutdown,
and persistence replay.

## 11. Performance contract

Benchmark semantics and matrices live in
[`src/benchmark.md`](src/benchmark.md). Benchmarks are not run during
correctness refactoring without explicit instruction.

Final acceptance uses clean, reconstructible baseline/candidate trees on the
same host/toolchain and an identical runner/harness fingerprint. Quick mode is
for focused diagnosis; a release verdict requires repeated medium/full records.
Runs use isolated target directories, alternating adjacent A/B order where
supported, median aggregation, and a noise-spread rejection gate.

The architectural performance gate is strict:

- throughput geometric mean must not decline;
- repeated or statistically significant scenario regressions block release;
- p95/p99 latency, CPU, allocation/RSS, dependent-chain, RBF, reorg, template,
  and shutdown behavior must not regress materially;
- workload, samples, safety checks, and timeouts may not be weakened to make a
  comparison pass.

Performance optimization may change indexes and batching, but not the
ownership, rollback, capacity, or effect invariants above.

One deliberate residual remains in template packing: after 4,000 consecutive
packages fail the remaining size/cycle budget, `TxSelector` stops scanning.
This bounds per-template adversarial CPU, but a crafted high-score non-fitting
prefix can cause bounded underfill and delay lower-score fitting transactions.
Removing the cap would restore an O(pool) attack surface. A future fix must use
a resumable cursor or a fit-aware indexed selector and pass both packing-
quality and CPU/RSS A/B gates; it is not silently classified as fixed here.

## 12. Frozen root-cause simplification

The production baseline for this simplification is commit
`53178b830f69fcdcc73ece2e0ea812d4357251bf`. The cutover at that commit is the
recovery point: later phases may simplify its encoding, but may not weaken its
ownership, rollback, resource, effect, reorg, or template guarantees.

The target is a thin pre-pool. Persistent state describes ownership and work
phase only. Scheduling, conflict preference, capacity eligibility, and queue
membership are derived facts which can be rebuilt and audited from entries.
No finding may add another lifecycle owner, persistent conflict wait state,
payload queue, or executable recovery channel.

### 12.1 Identity and resource domains

Three identities are deliberately distinct:

- lifecycle identity is the transaction hash; a trusted witness-bearing
  duplicate may replace the payload only through the typed promotion path;
- verification-cache identity is `TxVerificationCacheKey`, derived from the
  witness hash;
- proposal short ID is a bounded lookup key and never an ownership identity.

Resource invariants are domain-specific rather than one overloaded `charged`
predicate:

```text
entry residency(tx) <=> PipelineCoordinator owns tx
active work(tx)     <=> tx owns a current checked-out worker lease
effect charge(e)    <=> e is reserved, queued, or active in EffectOutbox
```

`AdmissionId` (the current `incarnation`) distinguishes remove/readmit cycles.
`LeaseRevision` (the current `revision`) invalidates work within one admission.
They have different semantics and must not be merged.

Entry residency and physical worker retention are deliberately separate
domains. Invalidating an entry drops every coordinator-owned resolved or
verified payload and retains only raw ingress. A stale worker may still hold
its checked-out `Arc` until it returns, but that bounded physical-worker tail
is not a current lease, cannot be looked up or committed, and does not justify
a second detached-work ledger.

### 12.2 Target lifecycle

The target persistent locations are:

```text
RawQueued(stage)
RawActive(stage, lease)
WaitingParents
VerifyQueued
VerifyActive(lease)
Verified
Committing(lease)
InvalidatedPending(raw-only)
```

Absence is `Nowhere`. `ReadyToCommit`, `WaitingConflict`, and
`ConflictRecheck` are deleted. Commit eligibility is not a lifecycle state.
Local RPC submission and reorg-retained replay remain synchronous direct
resolve/verify/submit paths by design.

### 12.3 Derived verified-conflict contract

One concrete `CandidateRank` supplies a stable total order for verified
conflict preference, conflict-capacity victims, and commit candidate ordering.
It does not replace the stage-specific `TicketQueue` ordering used by
pre-check, resolve, and verify work. Final RBF validity remains exclusively a
`PoolMap` decision under the `TxPool` write guard.

`StagedConflictIndex` is a rebuildable derived index containing input buckets
and, for every verified candidate, its distinct direct-conflict degree and the
number of directly conflicting candidates with a stronger rank:

```text
edge(a, b) <=> a and b are staged (Verified or Committing) and share an input
degree(v) == number of distinct direct neighbours of v
stronger_count(v) == number of direct neighbours ranked above v
eligible(v) <=> Verified(v) && stronger_count(v) == 0
eligible(v) <=> exactly one live commit ticket for v
```

A finite conflict graph without a committing member has at least one local
maximum; it need not have only one. Independent local maxima may remain
eligible concurrently. Checkout atomically exchanges the live ticket for
`Committing(lease)`. A committing member remains in the graph with frozen
priority above every verified neighbour until success or abort, but is never
itself represented by a queue ticket.

Any source or metadata change that changes `CandidateRank` is committed with
the corresponding conflict delta in the same coordinator mutex and undo
transaction. A short-lived `ConflictDelta` plans entry, degree,
`stronger_count`, ticket, victim-key, and entry-residency changes. It is not a
persistent store or a second owner. Capacity is reserved before applying the
delta; application is all-old, all-new, or fail-stop.

Successful pool handoff terminalizes the winner and its current direct staged
conflicts only. It never walks a transitive conflict closure. Failure settles
only the checked-out winner; the derived index makes all surviving eligibility
changes explicit without restore/waiter/recheck states.

Hot-path work is bounded by transaction inputs and the configured direct
candidate limit. Batch removal visits each affected direct edge once, refreshes
each surviving ticket once, and never scans all coordinator entries. Existing
owner-head queues, capacity/victim indexes, conflict-cache cursors, and effect
outbox bounds remain mandatory attack-cost controls.

### 12.4 Cancellation, promotion, and failure boundaries

`PipelineEpoch::advance` plus coordinator clear remains the linearizable
cancellation barrier. Scheduling source may promote Remote -> Local/Proposal;
immutable ingress attribution remains attached until exactly one relay
settlement. Missing-parent dependency edges and verified-input conflict edges
are different causal domains and may not share lifecycle state or wake rules.

Errors remain partitioned:

- stale lease or epoch: harmless no-op;
- malformed, duplicate, policy, or capacity outcome: typed transaction result;
- ownership/index contradiction or uncertain authoritative mutation:
  monotonic fail-stop.

The commit cutover reserves effect credit before locking, treats PoolMap
insertion as tentative, and publishes conflict history and effects only after
the coordinator handoff. A returned capacity-class handoff error is fully
undone, rolls PoolMap back from its exact journal, settles the commit lease and
may reject only that attempt. A returned invariant error follows the same
cleanup but latches Pipeline failure after the pool guard opens. A coordinator
panic already latched by `PipelineRuntime` is never re-entered: its entry undo
and the PoolMap journal preserve the pool recovery point, while a panic or
rollback failure with uncertain PoolMap mutation remains Authoritative.

Ordinary hostile input must not reach fail-stop. Genuine invariant failure
still stops the service generation; the accepted availability residual remains
documented in the security ledger.

### 12.5 Execution and convergence gates

The phase list is fixed; phase numbers and scope do not drift during execution:

| Phase | Deliverable | Whole-architecture gate |
|---|---|---|
| 0 | Frozen specification, evidence manifest, baseline | No production behavior change |
| 1 | Reference model, property tests, operation-count assertions | Model proves ownership, charge, eligibility, handoff |
| 2 | `CandidateRank`, derived conflict index/delta, old conflict lifecycle deleted | No dual production path or unbounded scan |
| 3 | Raw-only invalidation and token/API/generic simplification decision | Fewer states/APIs; RPC behavior explicit |
| 4 | Commit/error/effect boundary cleanup and production fault matrix | Ledger #104 and authoritative undo boundary are Covered |
| 5 | Semantic source/test split and dead-complexity removal | Production code is net smaller and easier to review |
| 6 | Full nextest/integration/security acceptance | No unexplained Partial or stale current evidence |
| 7 | Final architecture/attack review and checkpoint | Production-ready except explicit performance gate |
| 8 | Controlled checkpoint A/B | Deferred until explicit benchmark instruction |

After every phase, review the complete ownership graph, causal exits, lock/wait
graph, resource equations, attacker-controlled complexity, effects, RPC,
reorg/template/persistence behavior, security ledger, and production-code
delta. A failed phase is reverted to its checkpoint instead of being repaired
by adding lifecycle state. Only a P0/P1 correctness defect, exploitable resource
or service denial, invariant/authoritative uncertainty, template liveness, or a
measured performance regression may reopen the frozen architecture.

Current executable evidence is normalized in
[`review-behaviors.json`](review-behaviors.json), rendered for reviewers in
[`REVIEW_GUIDE.md`](REVIEW_GUIDE.md), and selected with the frozen inventory by
[`security-regression-manifest.json`](security-regression-manifest.json).
CI validates guide/seam drift and every Rust anchor against `cargo nextest list`
through the three `devtools/check_tx_pool_*` gates.
Historical names in the larger ledger are archival and are intentionally not
treated as compile-time anchors. Release mode additionally requires the
manifest's blocker list to be empty. Benchmarking remains outside phases 0-7.

### 12.6 Execution record

This table is append-only during the refactor so a resumed session can recover
the architectural state without reconstructing chat history.

| Phase | State | Evidence and correction |
|---|---|---|
| 0 | Complete (`4245c1a5d`) | Frozen contract, nextest-backed evidence manifest and Ubuntu CI check; no production behavior change. |
| 1 | Complete (`efc2702d3`) | Seven independent conflict-model tests; 310/310 nextest green. The model corrected the edge definition so `Committing` remains a frozen staged neighbour. No production path changed and no new clippy warning was added; 23 baseline lint findings remain for semantic cleanup/Phase 5. |
| 2 | Complete (checkpoint is the commit containing this row) | Replaced `ReadyToCommit`/`WaitingConflict`/`ConflictRecheck` and blocker/waiter indexes with one total `CandidateRank`, bounded input buckets, derived `(degree, stronger_count)`, and atomic `ConflictDelta` ticket reconciliation. A sole derived committing identity serializes independent maxima. Pre-eviction undo uses an input-local upper bound while final membership retains the direct-cohort bound. Global review found and closed the lost non-verify wake path with one level-triggered commit consumer; verify keeps only an eager fast path through the same serial driver. No new lifecycle state, recovery protocol, global scan, or second authority was added. 310/310 nextest green. |
| 3 | Complete (checkpoint is the commit containing this row) | Invalidated entries are now raw-only: typed payload ownership, typed lookup and conflict scheduling disappear in the same undo transaction, residency is recharged to the canonical raw equation, and stale worker completion is rejected by the existing lease revision. Deleted the redundant `PayloadPhase`/`PhaseMismatch` projection and changed typed lookups to borrowed views. Global review deliberately retained the three payload generics as compile-time phase isolation and retained `incarnation` plus `revision` as distinct admission/lease identities; adding aliases, a detached-worker ledger, or a compensating state would weaken reviewability. Production source is net smaller, no persistent state or hot-path scan was added, 311/311 nextest is green, and the clippy finding set remains the same 23-item baseline. |
| 4 | Complete (checkpoint is the commit containing this row) | Queue sequence allocation now precedes reservation and every fallible reservation is protected by existing entry undo; deleted the runtime's owner-map-wide post-transition cleanup, so reservation leaks are no longer hidden and the common mutation path loses an attacker-sized scan. The production fault matrix crosses reserved effects, tentative PoolMap insertion, a fully applied then undone coordinator handoff, exact pool rollback, required lease settlement and FIFO effect publication. Coordinator panic now stops the driver without re-entering failed state or falsely disabling an exactly restored pool recovery point; returned invariant errors settle and then fail-stop outside the pool guard. Stable-effect journaling remains under the pool lock for ordering but outside the Authoritative panic domain. Ledger #91/#104 are Covered, the dedicated release blocker is removed, 316/316 nextest is green, and the clippy set remains the same 23-item baseline. No lifecycle state, recovery queue or second authority was added. |
| 5 | Complete (checkpoint is the commit containing this row) | Split the coordinator by lifecycle, commit, maintenance, scheduling, capacity, undo, conflict-index, audit and type/queue semantics without changing its mutex or data layout; the physical entry file fell from 4,034 to 266 lines. Split the 4,876-line end-to-end test file into failure, identity, lifecycle, dependency, runtime, template and replacement suites with stable unique test anchors. Coordinator rejection policy now has one fail-safe classification table instead of three overlapping variant lists, and peer revocation checks committing state without materializing an owned diagnostic view. Whole-architecture teardown review found that a cancelled pre-check worker could checkout from a still-nonempty queue and keep the runtime alive; cancellation is now the pre-check dispatch barrier and has a direct regression. The historical 23-item clippy set is zero, nonblank/noncomment production Rust is net smaller than the Phase-4 checkpoint, 317/317 nextest is green, and no lifecycle state, allocation, task, lock, dynamic dispatch or hot-path scan was added. |
| 6 | Complete (checkpoint is the commit containing this row) | Closed the executable evidence against the production cutover: 319/319 isolated tx-pool nextest cases, 19/19 contextual-verifier cases, zero-warning all-target clippy, manifest validation, freshly rebuilt normal-mining reorg 3/3 and RBF success/failure/concurrency/recovery/attack 6/6. Ledger review removed stale prototype-era `Partial`/`Model-covered` labels and routes measurement-only work exclusively to O5. The RBF Proposed integration spec was corrected to require the level-triggered assembler to stop mining a rejected victim and normally propose/commit its replacement. Deterministic cross-authority query races prove clear/reorg cannot expose an ownership gap. The phase review also reproduced a clear-vs-reorg effect-credit deadlock: clear held ordinary credit while waiting for `recovery_lock`, whose owner needed ordinary credit for detached replay. Reorg/clear now share `recovery_lock -> effect credit -> TxPool`; no state, queue, task, scan or hot-path lock was added. |
| 7 | Complete (checkpoint is the commit containing this row) | Final attack review unified dependency semantics across resolver success/failure, coordinator invalidation, accepted PoolMap links/audit, RBF victim checks, historical recovery, chain/pool availability events and persistence. It closed bounded counterexamples for dep-group cache stranding, attached-parent lost wakeups, hostile-input fail-stop, stale resolved children, required-parent eviction, unbounded cell-ref mutation, dep-group RBF bypass, incomplete persistence ordering and nested late-parent weight drift. Conflict recovery now retains ownership until all indexed outpoints are live; PoolMap plans at most 100 physical displacements before mutation. The review corrected malformed remote policy to ban+record without an ineligible relay reject. No lifecycle state, executable queue, worker, lock, global hot-path scan or second ordering authority was added. Fresh acceptance: 328/328 internal nextest, 19/19 contextual verifier, zero-warning all-target clippy, manifest validation, normal-mining reorg 3/3 and RBF 7/7. Benchmarking remains Phase 8 only. |
| 8 | Deferred | Run only after explicit benchmark instruction. |

## 13. Semantic compaction and test-driven review program

The correctness cutover above is the safety checkpoint, not a claim that its
encoding is minimal. At checkpoint `172b9c935`, `tx-pool/src` contains 46,338
Rust source lines: 17,079 in separate test files, 1,689 more in eleven inline
test modules, 1,456 in the benchmark harness, and approximately 26,000 in the
release implementation after removing those inline test bodies. The pipeline
owns necessary complexity, but repeated transition/undo/effect scaffolding and
test setup still impose avoidable review cost.

This program compacts that encoding without reopening the lifecycle design.
The following mechanisms are frozen and cannot be removed merely to meet a
line target: the accepted `PoolMap` authority, the pre-pool coordinator,
non-executable bounded `ConflictCache`, reserve-before-mutation
`EffectOutbox`, distinct admission-incarnation and lease-revision tokens,
complete expanded dependency graph, per-peer/global resource budgets,
`recovery_lock`, and the block-assembler priority contract.

### 13.1 Baseline and evidence vocabulary

The frozen migration baseline is:

- 328 discovered `ckb-tx-pool --features internal` tests;
- 79 invariant-to-test references covering 59 unique Rust tests;
- 10 process-level source anchors;
- eleven inline `mod tests { ... }` bodies (about 1,689 lines);
- 34 production files containing at least one `cfg(test)` site;
- 17 named `*_for_test` functions.

Only explicit machine-readable behavior-registry entries are executable anchors.
Backticked names in the historical ledger are provenance, never anchors
discovered by prose regex or identifier-length heuristics. A generated test
inventory protects the physical-move and compaction phases from accidental
test deletion or rename. The validator reports reference count, unique unit
evidence, integration evidence, and total discovered tests separately.

### 13.2 Test isolation and white-box policy

Test bodies live in domain test files (`component/tests`, `service/tests`,
`process/tests`, `block_assembler/tests`, and root-module test files). A
production source may contain only one-line test-module wiring and an audited
minimal seam. Physically moved tests remain logical child modules so Rust
privacy is preserved; production visibility must never be widened only for a
test. Prefer public-contract assertions. When an internal invariant genuinely
requires white-box observation, use one `cfg(test)` probe/snapshot surface for
the owning type rather than scattered field accessors.

Every retained test-only field, accessor, fault seam, and inspection probe is
listed by stable file, symbol, kind, and behavior ID. Line numbers are not
stable identities. CI rejects inline test bodies, test functions outside the
allowed test tree, and new or changed seams absent from that whitelist. Fault
injection remains test-only and cannot add a production state or dynamic hook.

### 13.3 Test-driven PR review guide

`REVIEW_GUIDE.md` is the human entry point for future tx-pool changes. Its
behavior table assigns stable `TP-*` IDs and records the change surface,
required behavior, hostile/failure counterexample, I1-I12 invariants, reviewer
questions, minimum unit/model command, integration specs, and performance
bound. A normalized manifest registry owns the test/spec mapping, and the
guide's evidence appendix is generated from it; two handwritten evidence
lists are forbidden.

A reviewer starts with the changed path/API, follows the mapped behavior rows,
runs their minimum tests, and then applies the cross-authority gate when a
change touches ownership, lock order, accounting, reorg, effects, persistence,
template liveness, or an attacker-controlled bound. A behavior change must
update the guide, registry and focused negative regression in the same PR.

Benchmark credibility is part of the guide even while timing is deferred.
`TP-PERF-*` covers the Rust workload harness and the deterministic runner in
`devtools/tx_pool_bench.py`, including isolated Cargo targets, harness
fingerprinting, paired-worktree comparison, repetition requirements and noisy
spread rejection. These are functional harness tests, not performance runs.

### 13.4 Compaction rules

Physical test movement is behavior-neutral and must preserve the exact test
inventory. Test-code compaction may share fixtures, case runners and model
operations, but stable named regressions, failure locality and every current
security anchor remain. A large opaque mega-test is not a valid reduction.

Production compaction is accepted only in independently reviewable
net-deletion slices. A new abstraction must delete at least two parallel
encodings and cannot add a lifecycle state, owner, queue, worker, lock,
mutable projection, global hot-path scan, dynamic dispatch boundary, or
compensation protocol. Splitting a large file is organization, not semantic
compression. The independent test audit must not reuse the production rebuild
algorithm, because common-mode reconstruction would weaken its evidence.
Likewise, heterogeneous `try_reserve` sites may be grouped only when their
container, failure timing and charge equation are identical.

### 13.5 Fixed phases and exit gates

| Phase | Deliverable | Mandatory exit gate |
|---|---|---|
| S0 | Freeze plan, exact test inventory, evidence terminology and current size/seam baselines | No production behavior change; manifest/list/checker agree |
| S1 | Normalize historical ledger evidence | Every historical row is live, `guarded_by`, historical, accepted or superseded; prose is not executable evidence |
| S2 | Physically isolate test bodies and helpers | Exact test names/count preserved; no inline tests; no visibility widening; seam whitelist complete |
| S3 | Publish Review Guide and normalized behavior registry | Every current evidence/spec maps to a `TP-*` row; generated appendix and CI drift checks pass |
| S4 | Compact test encoding | Stable anchors and failure locality preserved; total test source is materially smaller |
| S5 | Compact production encoding | Each slice is net-negative production code and passes the whole-architecture gate |
| S6 | Full correctness/security acceptance | nextest/contextual/integration/clippy/format/manifest gates green; residuals documented |
| S7 | Controlled checkpoint A/B | Deferred until explicit benchmark instruction |

After every phase, review the complete ownership graph, causal exits, lock/wait
graph, resource equations, attacker-controlled work, effect order, RPC,
reorg/template/persistence behavior, evidence mapping and source delta. If any
gate fails, revert to that phase's checkpoint. Do not patch forward on top of
a failed design. Correctness or safety defects discovered by the review may be
fixed only within an existing authority and must not bypass the net-complexity
and architecture gates.

### 13.6 Compaction execution record

| Phase | State | Evidence and correction |
|---|---|---|
| S0 | Complete (checkpoint is the commit containing this row) | Safety checkpoint `172b9c935`; exact 328-test inventory, evidence terminology, source/seam counts and semantic-compaction gates were frozen before any physical move. The manifest validator rejects missing, renamed, duplicate and unrecorded tests; release validation passes with 79 invariant references, 59 unique Rust tests and 10 process-level anchors. No production behavior changed. |
| S1 | Complete (checkpoint is the commit containing this row) | Historical evidence was compared with the current coordinator/pool/outbox architecture. Ten stale legacy test labels now point to current tests or explicitly state that the vulnerable mechanism was deleted. Four properties that remain source-enforced but lacked an exact current counterexample—active peer revocation, expiry cascade, save/reorg serialization and zero-worker clamping—are marked `Guarded by current boundary` and are mandatory S3 regressions rather than falsely reported as Covered. Whole-architecture review found no ownership, lock, accounting, effect, reorg/template or attack-surface change because this phase changed evidence only. |
| S2 | Complete (checkpoint is the commit containing this row) | Physically moved every inline test body and white-box helper into declared test roots while preserving all 328 frozen baseline names. CI now rejects inline tests, unreviewed module wiring, visibility drift and undeclared seams; the release implementation retains only 31 module wires, 79 `cfg(test)` sites and 32 named, behavior-tagged seams, with no production visibility widening. Whole-architecture liveness review of every production `Notify`, waiter and checkout found that queue-nonempty readiness could self-wake an active-cap-blocked peer, a shared verify permit could be consumed by `SmallCycleOnly` while only large work existed, a respawn could inherit a consumed permit, and outbox `notify_waiters` had a check-to-sleep registration window. Readiness is now derived from the authoritative capability-aware checkout predicate and re-armed on subscription; outbox waiters register before checking. Five explicit regressions increased the intentional inventory to 333. Fresh gates: 333/333 internal nextest in 24.514s, zero-warning all-target clippy, format/diff/layout checks, and 90 invariant references covering 64 unique Rust tests plus 10 integration anchors. No lifecycle state, executable owner, work queue, lock, compensation path or population-sized hot scan was added; benchmark execution remains deferred. |
| S3 | Complete (checkpoint is the commit containing this row) | Added four hostile-boundary regressions that were previously only source-guarded: active peer revocation refunds its live lease/budget, reorg expiry cascades through fresh descendants, persistence waits for complete detached replay, and a configured zero verify-worker count still executes and shuts down a remote pipeline. `review-behaviors.json` is now the sole normalized mapping for 15 stable `TP-*` behaviors, 75 unique Rust tests / 112 invariant references and 10 process specs; it generates the evidence section of `REVIEW_GUIDE.md`. CI rejects guide drift, unknown test-seam behavior IDs, renamed/missing/new inventory tests and stale I1-I12/source anchors. The readiness/lost-wakeup family is permanently assigned to `TP-WORKER-001` and `TP-EFFECT-001`, with explicit hostile cases and focused commands. Migration comparison proved all 64 prior Rust evidence anchors and all 10 prior specs remain. Whole-architecture review found no production state, owner, lock, queue, worker, visibility, hot scan or behavior change in S3. Fresh gates: 337/337 isolated nextest in 25.696s, zero-warning all-target clippy, format/diff and all three evidence/layout checks. Benchmarking remains S7 only. |
| S4 | Pending | — |
| S5 | Pending | — |
| S6 | Pending | — |
| S7 | Deferred | Run only after explicit benchmark instruction. |
