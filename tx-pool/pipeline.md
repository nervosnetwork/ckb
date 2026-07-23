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

### Cross-authority order

An operation that needs accepted and pre-pool state takes the async `TxPool`
guard first, then the coordinator's short-held synchronous mutex. Coordinator
code never acquires `TxPool`, and its mutex is never held across `.await`.
Combined queries use the same order, so they cannot observe a transaction in
neither owner or in both owners during a handoff.

`recovery_lock` is outside `TxPool` and is used only to exclude persistence or
another chain-wide recovery from an incomplete retained-transaction replay.
Normal remote commits do not take it. Effect capacity is reserved before
`recovery_lock` or `TxPool` is acquired.

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
  -> ReadyToCommit | WaitingConflict | ConflictRecheck
  -> Committing
  -> TxPool or terminal record
```

Each worker receives a lease containing incarnation/revision evidence. A stale
finish cannot modify a newer owner. Checkout and completion preflight sequence
capacity before consuming the only live ticket. Source promotion is a typed
transition, not a second entry.

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

The coordinator indexes child IDs by full parent hash. A missing-parent
transition and its remote `UnknownParents` effect are committed together.
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

`ConflictCache` is a bounded, non-executable store inside `TxPool`, indexed by
short ID and input outpoint. Count and serialized-byte caps bound retained
payload. Generation-tagged FIFO tickets and bounded-ratio compaction prevent
remove/reinsert churn from growing stale metadata or letting an old ticket act
on a new incarnation.

Every pool mutation that frees inputs—RBF, administrative removal, and reorg
reconciliation—marks fully unblocked cache entries in the same `TxPool` write
transaction. The shared maintenance worker drains at most 32 per slice:

1. reserve terminal-effect capacity;
2. acquire `TxPool`;
3. pop one generation-valid cache candidate;
4. recheck accepted-input conflicts;
5. admit it to the coordinator while the pool guard remains held;
6. remove the cache copy only after coordinator admission succeeds.

Thus the cache is sole owner before the handoff and the coordinator is sole
owner after it. `Full` reschedules the same cache generation and retries on a
coarse tick without a hot loop. A newly arrived accepted blocker leaves the
candidate cache-owned and unscheduled until that blocker later frees the input.

There is no deferred recovery channel and no publication barrier. The only
bounded asynchronous auxiliary channel carries best-effort verification-cache
updates; executable transactions never pass through it.

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

The coordinator uses generation/version tickets and compacts physical
tombstones to a bounded ratio. The conflict cache independently compacts both
insertion and recovery tickets. The effect outbox continuously charges
reserved, queued, and active batches, so moving a terminal payload out of a
state owner does not create an uncharged backlog.

Known final-audit item: some coordinator priority/capacity victim discovery
still scans the authoritative live set. Semantics are bounded by victim and
graph limits, but Stage 6 must replace repeated whole-store selection with an
equivalent derived index or prove the operation count and performance budget.

## 7. Stable-state effects

Callbacks, relay results, peer bans, and reject notifications are immutable
`TxPoolEffect` records. Producers reserve bounded outbox capacity before state
mutation and commit the exact batch at the mutation boundary. A single
publisher preserves FIFO batch order, retries a full relayer without dropping
the active head, and runs endpoint code outside state locks.

Callback panics are contained by the publisher. A callback may issue read-only
controller queries. Synchronous mutating controller re-entry fails fast because
waiting for a mutation from the publisher can form a cycle through outbox
capacity or `recovery_lock`. Each callback observes a completed local
pool/coordinator mutation; during multi-step detached replay, read-only RPC may
observe the current stable reorg slice while `recovery_lock` still excludes
persistence.

Block-assembler notification uses a level-triggered dirty bit plus a bounded
wake channel. A full channel may coalesce a wake edge but cannot erase the
authoritative Pending/Proposed/Uncle/Reset work.

## 8. Reorg and block-template consistency

The reorg handler retains the FIFO head delta across panic/retry; later deltas
cannot overtake it. Before mutation it reserves critical outbox headroom that
ordinary traffic cannot consume. Under `recovery_lock`, the pool and
coordinator apply attached commits, detached/unavailable parents, status
changes, conflict recovery scheduling, and terminal effects in their defined
lock order. Detached replay is topological and compares attached identity by
raw transaction hash.

Reorg status reconciliation explicitly demotes stale `Gap` entries when the
new proposal window no longer justifies them. Block-template proposal
selection also prevents optional detached-block uncles from suppressing the
only proposal path for recovered transactions. The regression mines a
parent/child/grandchild tree through normal `get_block_template`; it does not
inject a manual proposal block.

Template updates use one revisioned template and a serialization lock for full
reset/full update interactions. Proposal byte accounting uses the consensus
block-size basis. Uncle selection is capacity bounded, stale/main-chain/
embedded candidates are removed, and optional uncle content cannot strand a
valid pool transaction.

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
3. if any worker timed out or panicked, abort remaining work and skip
   persistence;
4. close and drain the effect outbox;
5. persist only after all state/effect boundaries completed.

This prevents a half-recovered or effect-incomplete pool from being saved as a
clean shutdown image.

## 10. Invariant and regression gates

The coordinator invariant audit reconstructs full-hash, short-ID, peer,
dependency, conflict, queue, deadline, active-work, and residency indexes from
typed entries and checks physical live-ticket equality. Transition fault
injection verifies undo restoration across multi-entry changes.

A stage is complete only after both reviews pass:

- incremental review: the stage's code and focused regressions;
- whole-architecture review: ownership, causal exits, lock/wait graph,
  state→effect atomicity, capacity/attack cost, RPC/template/persistence
  consistency, algorithmic complexity, readability, and new regressions.

Required correctness commands near a checkpoint:

```bash
cargo test -p ckb-tx-pool --features internal -- --test-threads=1
cargo fmt --all -- --check
git diff --check
```

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
