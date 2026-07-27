# Tx-Pool Pipeline

This is the implementation map for the architecture frozen in
[`ARCHITECTURE.md`](ARCHITECTURE.md). It explains the ordinary data flow and
where reviewers should expect each proof. Current execution status and
checkpoint history live in [`IMPLEMENTATION_PLAN.md`](IMPLEMENTATION_PLAN.md).

## 1. Scope

The pipeline applies to Remote, Proposal and detached-chain Recovery traffic.
Local RPC submission intentionally runs the direct synchronous
resolve/verify/commit path and returns its result. Both paths converge at the
same accepted-pool immutable Plan/total Apply boundary.

There are two payload authorities:

```text
PrePoolKernel  --successful commit handoff-->  TxPool
```

No queue, worker, conflict cache, effect batch, reorg handler or block template
owns an executable transaction.

## 2. Ordinary remote/proposal flow

```text
controller message
  -> non-contextual verification + cross-authority duplicate fence
  -> PrePoolKernel admission and continuous budget charge
  -> ResolveQueued(Ingress or Ordered)
  -> versioned ResolveLeased
     -> Wait(Missing/Conflict), or
     -> VerifyQueued(resolved payload)
  -> versioned VerifyLeased
  -> script verification + fee gate
  -> Ready(verified payload, inputs, total rank)
  -> single commit driver
  -> accepted Plan
  -> kernel handoff + total PoolMap Apply + stable effect append
  -> callback/relay/template work outside authority locks
```

Remote and Proposal work may resolve/verify concurrently. Accepted commits are
serialized because `PoolMap` mutation and final RBF/ancestry status are one
authoritative order. Serialization begins only at Ready; it does not remove the
pipeline's expensive verification parallelism.

## 3. Local direct flow

```text
local RPC
  -> non-contextual verification
  -> resolve against snapshot/pool overlay
  -> script verification (cache keyed by witness hash)
  -> accepted Plan
  -> settle matching pre-pool owner
  -> total PoolMap Apply + stable effect append
  -> return result and publish effects
```

Local does not queue and does not create a speculative competing owner. A local
duplicate can rebroadcast an already accepted transaction; a matching retained
Remote owner is settled under the accepted write guard so it cannot later
commit twice.

## 4. PrePoolKernel

Implementation: `src/component/pre_pool/`.

### Primary entry

The primary map is keyed by full transaction hash. An entry contains compact
raw payload, source, one typed state, non-reused `u128` version, arrival,
deadline, charge and canonical dependency keys.

The six locations are:

```text
ResolveQueued
ResolveLeased
Wait(Missing | Conflict)
VerifyQueued
VerifyLeased
Ready
```

Recovery is a source. Missing and Conflict are Wait reasons. They are not extra
locations.

### Projections

`FairQueue`, `by_short_id`, `by_ingress_peer`, `by_parent`, `waiters`,
`deadlines`, `ready`, `ready_by_input`, usage and active-work maps are derived
in the same mutex section as the primary. `by_ingress_peer` is immutable
revocation attribution; mutable scheduling source and its remote/per-peer
budget may be promoted independently. A peer ban therefore removes every
matching owner still in `PrePoolKernel`, including source-promoted work, while
an already accepted `TxPool` entry remains authoritative. Its terminal Reject
also clears the relayer known/pending projection so another peer can announce
the same hash. The expiring, non-evicting ban marker is checked immediately
after queued remote admission takes ownership and again from the exact Ready
ticket before Accepted planning. Its cardinality follows the network's
existing unexpired ban set; unrelated peer churn cannot evict a still-live
fence. Removal and both race edges reuse the same immutable ingress fact rather
than adding a ban lifecycle state. Tests recompute kernel projections
independently.

### Single-entry transitions

Hot transitions validate the final entry and exact usage/active delta before
moving the primary. They use stack-sized plans; they do not allocate a generic
cohort snapshot.

### Cohort transitions

Commit settlement, parent loss, bounded recovery and wait cascades can update
multiple primaries. A private bounded `MutationSet` derives one exclusive
`PreparedKernelMutation<'_>` that validates one final primary per hash,
short-ID uniqueness, fan-out and exact final counters. Only consuming
`commit(self)` moves entries and projections; callers cannot mutate the kernel
between Plan and Apply, clone old entries for undo, or roll back.

### Leases

Resolve/verify leases carry full hash, exact version and immutable `Arc`
payload. Checkout advances the version and active counters. Completion mutates
only if hash/version/location still match. Clear, promotion, parent loss or
re-admission makes old work stale without reusing an identity.

## 5. Resolve, wait and verification

Implementation:

- `src/service/stages/resolve.rs`
- `src/service/stages/verify.rs`
- `src/service/workers.rs`
- `src/process/classify.rs`
- `src/process/submit/`

Resolve lanes distinguish ordinary ingress from parent-first ordered work.
Unknown/dead dependencies are converted into exact `DependencyKey` sets.
`Wait` keeps one observed availability epoch per key; dirty keys are drained in
bounded maintenance slices. A missed wake notification cannot erase the level.
At the unique cohort seal, a `Wait(Conflict)` owner created by that same Apply
is bound to the post-Apply dependency cut, while unchanged historical conflict
owners retain their older observation and wake. `Wait(Missing)` is intentionally
not rebased because definitive parent loss schedules bounded re-resolution
through the same level change.

Verification uses the declared Remote cycle cap where applicable and checks
the exact snapshot generation. The cache key is
`TxVerificationCacheKey::from_transaction`, i.e. witness hash. A resolved
payload is compacted so a small retained transaction cannot pin large block or
cell backing allocations outside its charge.

Resolver and verifier jobs return typed computation outcomes. Internal worker
logic does not use `panic + catch_unwind` to select settlement, retry or
shutdown behavior. Genuinely foreign callbacks/endpoints execute outside
authority locks behind a thread/task/channel boundary, so their failure or
hang becomes a typed channel/timeout outcome rather than an unwind-driven
state transition.

## 6. Ready and commit

Implementation:

- `src/component/pre_pool/commit.rs`
- `src/process/submit/rbf_commit.rs`
- `src/component/pool_map.rs`

Ready order is source priority, exact fee rate, absolute fee, earlier arrival,
smaller full hash, version and size. One commit driver takes an exact
`CommitTicket`. Later stronger arrivals remain Ready for the next iteration and
do not invalidate the selected ticket.

Under the accepted write guard, submission:

1. computes current RBF closure and all policy checks;
2. calls `PoolMap::plan_mutation` to simulate removals/capacity and post totals;
3. settles the matching Ready owner and consumers of planned-unavailable
   parents in `PrePoolKernel` while accepted membership is unchanged;
4. calls `PoolMap::apply_mutation`, which only moves prevalidated entries;
5. appends the stable effect batch in the journal's innermost section.

Every legal rejection and every detectable structural fault occurs in steps
1–2. Private constructors and the exclusive prepared transaction make the
remaining state unrepresentable; steps 3–5 are total and contain no assertion
or unwind boundary. There is no nested undo or failed-winner restore.

## 7. RBF and conflict history

RBF closure includes exact input conflicts and required descendants and is
bounded before traversal by the full input×candidate product. The replacement
must pass both pool-level RBF policy and pre-pool size-based fee gates.

An unsuccessful verified conflict may be retained in `Wait(Conflict)` as
optional bounded history. It remains charged and owns its original immutable
source attribution. If history capacity is unavailable it is terminalized;
optional observability cannot block a valid winner. Availability of every
conflict key is rechecked before it wakes through ordinary resolution.

## 8. Effects

Implementation: `src/service/effects.rs`.

The journal contains immutable callback, relay, ban and recent-reject effects.
Capacity is partitioned Remote/Trusted/Critical. Remote cannot consume trusted
or chain-critical headroom.

`wait_capacity` is only a level-triggered hint and holds no reservation. Under
state locks, ordinary `try_apply` checks one exact prebuilt batch, executes
total Apply and appends one sequence. Callbacks, network sends and database
writes execute later outside state locks.

Only chain/admin authority may replace saturated detail with a prebuilt,
replaceable `GenerationReset` record; ordinary admission stays mutation-free
on `Full` and replans after an exact-capacity hint. A full relay endpoint
retains/coalesces authority rather than dropping the only wake.

Callback, network-ban and recent-reject database endpoints share a production
timeout and stable circuit per endpoint kind. Accepted-duplicate success holds
an accepted-membership read capability through append, preventing a stale
`Ok` from overtaking clear/reorg's removal and `GenerationReset`.

## 9. Reorg and administrative flow

Implementation:

- `src/process/reorg.rs`
- `src/component/pre_pool/recovery.rs`
- `src/service/pipeline_ops.rs`
- `src/persisted.rs`

Reorg runs chain mutation once under the accepted write guard. It combines
detached transactions with the accepted descendant closure of detached
producers, sorts one recovery set parent-first and applies one trusted kernel
plan. Those entries use the ordinary six states and `Recovery` source.

If the authoritative recovery closure exceeds the frozen representation bound,
the accepted/pre-pool ephemeral generation is reset rather than keeping an
unsafe child suffix. There is no generic phase replay, cross-await recovery
lock or handler-local retained payload.

Clear advances the pipeline epoch and swaps the pre-pool generation while
holding the accepted guard. Old population is destroyed after the lock.
Persistence takes the same administrative ordering and saves dependency-ordered
accepted/recovery-relevant transactions only after supervised workers quiesce.

## 10. Block assembler

Implementation: `src/block_assembler/` and the assembler loop in
`src/service/builder.rs`.

Reset/`update_full` share one ordered full-template publication boundary while
their construction remains concurrent. `update_full` has priority and derives
uncle content from the bounded candidate authority instead of copying the
reset template's transient projection. Uncle, proposal and transaction changes remain
concurrent optimistic versioned OCC generations; losing an individual channel
edge does not lose the dirty level. Every successful full/reset replacement
reissues all three partial generations so a racing stale acknowledgement
cannot erase work hidden by the replacement.

After reorg, accepted Gap status is reevaluated against the new proposal
window. Full and uncle-only plans prepare candidate uncles read-only; candidate
cleanup occurs only with the matching publication token. Candidates that would
suppress proposal IDs needed by recovered transactions are filtered.
Production/test limits are identical: 128 total candidates and 10 per height.

The unified loop supports:

- normal interval: immediate first tick, periodic reset/delta application and
  miner notification after progress;
- zero interval: eager delta application, one-second reset retry and no
  external miner notification;
- failed/superseded reset: hard barrier retaining ordinary deltas until the
  latest full rebuild succeeds.

## 11. Controller and query behavior

RPC/query lookup checks accepted and pre-pool ownership under the universal
accepted→kernel order. Pre-pool locations project to compatibility statuses;
internal Gap may still appear RPC-pending, so liveness tests use detailed entry
status where distinction matters.

Proposal filtering treats any accepted or pre-pool owner as known because the
same locations are searched for compact-block reconstruction. Full hash remains
authoritative; proposal short ID never aliases a collision into success.

`NotifyTxs(Vec<_>)` is a trusted controller input. Element validation and
pre-pool accounting are bounded; P5/P6 must retain an explicit residual until
the caller-side vector bound is proven.

## 12. Lock and await rules

```text
capacity hint (released)
  -> TxPool RwLock
    -> PrePoolKernel Mutex
      -> EffectJournal Mutex
```

Shorter kernel-only transitions are allowed. Reverse acquisition is not.
Await/I/O/callback/payload destruction does not occur under nested authority
locks. Full block-template serialization is independent and does not acquire
the transaction authority chain in reverse.

## 13. Testing and review

Production modules contain only `cfg(test)` wiring, observation or named fault
seams listed in `test-layout-manifest.json`; test bodies live in dedicated test
files. `review-behaviors.json` is the machine-readable mapping from architecture
behavior to unit/model and process regressions. The generated section of
[`REVIEW_GUIDE.md`](REVIEW_GUIDE.md) is the reviewer-facing table.

The required sequence is:

1. `cargo clippy -p ckb-tx-pool --all-targets --features internal -- -D warnings`
2. `cargo nextest run -p ckb-tx-pool --features internal`
3. document/test/security validators
4. complete managed process suite via `make integration`
5. checkpoint A/B benchmark only after correctness and harness-noise review

The P6.5 candidate completed steps 1–4: 257/257 `nextest` tests passed,
all-target clippy and static/document validators passed, the complete
150-spec managed tx-pool-impact universe passed in its recorded serial run,
and the repository-wide unfiltered 177-spec process universe passed through
plain `make integration` in 372.452 seconds.
Step 5 remains frozen pending explicit benchmark authorization.

## 14. Implementation checkpoints

| Checkpoint | Meaning |
|---|---|
| C0 `35cabc9b7` | pre-redesign coordinator baseline |
| C1 `02e648255` | audited fixes and rollback/A-B base |
| C2 `8596c6c5d` | formal target/evidence base |
| C3 `1d9e0cf5b` | six-state `PrePoolKernel` cutover |
| C4 `7219778be` | accepted immutable Plan/total Apply |
| C5 `74a5049bd` | statically partitioned effects |
| C6 `d9aac44e4` | reorg recovery and chain convergence |
| C7 `e3ab95375` | generation-swap disposal |
| C8 `473b4e927` | supervision and persistence eligibility |
| C9 `693413cf6` | recovery/failure boundary simplification |
| C10 `1e0d0098d` | bounded total cohort transitions |
| C11 `77dcbb0c1` | durable relay reconciliation |
| C12 `015d88be2` | Rust-native invariant outcomes |
| C13 `6d0577ad4` | move-only Apply and redundant-envelope removal |
| C14 `288031ebc` | six-state production contract and evidence acceptance |
| C15 `64ecdd0eb` | complete correctness/liveness and 150-spec integration acceptance |
| C16 `eb26bd272` | evidence checkpoint before exact-admission/static-authority convergence |
| C17 `9e559a482` | P6.5 exact admission, typed authority and full correctness acceptance |
| C18 `dd95e1f99` | post-regression correctness freeze before mechanical review-layout cleanup |

Checkpoint history is evidence for recovery and A/B, not a list of mechanisms
that remain in the final architecture.
