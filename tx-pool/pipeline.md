# Tx-Pool Pipeline — Current Implementation and Migration Record

This document maps the normative design in
[`ARCHITECTURE.md`](ARCHITECTURE.md) to production code. The independent design
review is [`ARCHITECTURE_AUDIT.md`](ARCHITECTURE_AUDIT.md), staged gates are in
[`IMPLEMENTATION_PLAN.md`](IMPLEMENTATION_PLAN.md), and executable review
evidence is generated in [`REVIEW_GUIDE.md`](REVIEW_GUIDE.md).

Status: **P3 stable-effect cutover complete at C5**. The old
`PipelineCoordinator`, `PipelineRuntime` and `ConflictCache` have been deleted.
PoolCommitJournal, nested restoration and persistent cell-ref ancestry are now
also deleted. P3-P4 migration debt is named explicitly below; it must be removed by the
corresponding phase rather than copied into the new kernel.

## 1. Authority and ownership

There are exactly two executable transaction owners:

| Authority | Owns | Does not own |
|---|---|---|
| `TxPool` | accepted `Pending`, `Gap` and `Proposed` entries | unaccepted work, historical conflicts, worker leases |
| `PrePoolKernel` | every admitted remote/proposal transaction before accepted-pool handoff | local submissions, accepted entries, effects |

`PrePoolKernel.entries: HashMap<Byte32, Entry>` is the sole pre-pool payload
map. Short-ID, owner, parent, Wait, deadline, work and Ready structures contain
only identities/versioned keys and are synchronously derived from that map.
Workers borrow immutable `Arc` payloads; a borrow is charged active work, not a
third owner.

Local submission remains synchronous by design. It borrows bounded service
resources, resolves/verifies directly and returns the authoritative result. If
the same hash has a remote/proposal pre-pool owner, settlement occurs under the
universal `TxPool -> PrePoolKernel` order.

## 2. Closed state model

The kernel has exactly seven locations:

```text
RecoveryRetained
ResolveQueued -> ResolveLeased
Wait(Missing | Conflict)
VerifyQueued  -> VerifyLeased
Ready
```

`RecoveryRetained` is frozen in the model and is connected to detached replay
in P4. It is not emulated with a temporary queue in P1.

There is no `Committing`, `Invalidated`, `RaceLost`, victim-hold or inferred
active state. Final commit uses an immutable `CommitTicket` that names the
current Ready rank and version. Dependency loss moves any queued, leased,
verified or Ready entry directly to `Wait(Missing)`, making every old lease
stale in the same kernel transition.

## 3. Identity and stale-work rejection

- Ownership keys are full raw transaction hashes.
- Proposal short IDs are collision-detecting indexes, never owners.
- One checked global `u128` clock issues a non-reused version for every live
  transition and re-admission.
- Resolve, verify and commit completions carry the version they checked out.
  Missing hash, different version, wrong location or different Ready rank is a
  typed stale result and cannot mutate the current owner.
- Verification cache access uses `TxVerificationCacheKey`, which is based on
  the exact witness hash. Reorg recovery never substitutes raw hash identity.

Proposal promotion changes scheduling/accounting attribution without trusting
the ordinal representation of peer IDs. A different trusted witness variant
replaces the payload at the authoritative handoff and invalidates stale work.

## 4. Continuous resource accounting

Every resident entry is charged by:

```text
current retained payload bytes
+ fixed primary/index overhead
+ canonical exact DependencyKey storage
+ conservative derived-index and future Wait reservation
```

The kernel maintains checked count/byte partitions for total, remote,
per-peer and historical-conflict residency. Active resolve/verify borrows have
global and per-owner limits. Dependency fanout, dependencies per entry, Ready
inputs and candidates per input are independently capped.

A remote Conflict owner remains charged to both remote/per-peer and conflict
partitions and retains its ordinary remote deadline. Wait reserves its exact
waiter/epoch/dirty-cursor footprint before invalidation; moving Wait back to a
resolve queue cannot require new capacity. Witness-variant replacement charges
the replacement payload, never the displaced witness size.

Admission and every recharge compute the prospective charge before replacing
the primary entry. A capacity rejection leaves primary state and all derived
views unchanged. Conflict history is optional armor: when its partition is
full, the rejected owner is terminalized rather than panicking, evicting
unrelated executable work or escaping to service fail-stop.

The limits currently derive from `TxPoolConfig`, consensus block size and the
existing bounded pool-mutation cohort. P5/P7 must validate the memory equation
and allocator/RSS consequences; timing benchmarks remain deferred to P7.

## 5. Scheduling and readiness

`FairQueue` keeps one runnable head per source owner. A capped owner contributes
no head, so checkout is bounded by owner heads rather than scanning an
attacker-controlled prefix. Successful service advances that owner's turn;
initial service cannot repeatedly favor the first peer. Proposal work retains
trusted priority, while remote peers receive round-robin opportunities subject
to active limits.

Readiness notifications are level-triggered hints. After every kernel mutation
the shell recomputes capability-aware readiness from authoritative heads and
notifies matching workers. Consuming or losing a notification does not remove
work. Small-cycle workers cannot consume the sole wake for ineligible large
work. The commit consumer reads the authoritative Ready level before sleeping,
so a panic that consumed a Notify but retained the owner retries after bounded
backoff without waiting for an unrelated mutation.

Verify ordering uses the configured policy. Every total order includes source,
fee policy, arrival, full hash, version and the remaining schedule fields, so
two distinct keys never compare equal accidentally.

## 6. Dependency and historical-conflict liveness

`DependencyKey` distinguishes an exact cell outpoint from a header hash.
Resolver and verifier `Unknown(outpoint)` results register that exact outpoint;
they do not replace the witness with every declared parent hash. Successful
dep-group expansion adds every discovered member to the entry's canonical
`DependencyKey` set. Input, cell-dep, header and expanded keys survive every
payload/state transition; `by_parent` is derived from those keys rather than
being a second, less precise causal fact.

Both missing dependencies and rejected conflict history use `Wait`:

1. the entry stores the exact key and the key's observed availability epoch;
2. `waiters` is a versioned reverse projection;
3. an availability change advances the key epoch and enqueues one dirty-key
   cursor;
4. bounded maintenance wakes only entries that observed an older epoch;
5. a concurrent newer change records a pending epoch and starts one follow-up
   pass after the current cursor finishes;
6. waking always re-resolves against current `TxPool`/chain state.

This is level-triggered. Re-waiting at the current epoch does not immediately
wake again, preventing missing-parent and dep-group resolution livelocks.
Epoch/dirty records exist only while a waiter or an in-progress cursor exists;
availability for a key with no waiter retains no permanent history. A later
parent loss unions with an existing Wait set, so sequential expanded-parent
loss cannot erase the earlier exact dependency.
Ready deadlines are filtered before the maintenance slice limit so skipped
Ready entries cannot starve later eligible expirations.

## 7. Verification and Ready commit

Resolution reads accepted-pool/chain state concurrently and publishes only a
version-checked immutable `ResolvedTx`. Verification performs contextual fee,
cycle, conflict and snapshot checks, then publishes a compact verified payload
and exact input set.

`ReadyKey` is the sole provisional preference order. It compares trusted
source, fee rate without division, stable arrival/full-hash ties and version.
`ready_by_input` is a derived index. Only Ready entries participate in pre-pool
conflict preference; an unverified high-fee transaction cannot displace or pin
verified work.

One async commit serial chooses the best current Ready ticket. Under the
existing `TxPool` write guard, final RBF, role-aware liveness, status, causal
ancestry and serialized/resident capacity decisions compile into one immutable
`PoolMutationPlan`. A sparse overlay reproduces the exact Pending -> Gap ->
Proposed eviction policy and re-ranks affected ancestors after each bounded
closure without cloning PoolMap. The versioned kernel handoff completes while
accepted membership is unchanged; PoolMap Apply then consists only of
prevalidated moves and assertions. There is no ordinary rollback path or
persistent committing state.

Accepted identity is a unique full-hash index plus a collision-detecting
proposal slot. The persistent graph contains only causal accepted producers of
inputs and expanded deps. Cell-dep reader -> spender is conditional consensus
ordering: it is added only to the already selected template set and
topologically ordered with stable selection-rank ties. Exact SCCs prevent
acyclic downstream entries from being misclassified as cycle members; every
round sheds the weakest member of each cyclic SCC and its causal descendants,
with a 64-round dense-SCC fallback. A dep-heavy entry that exhausts the
ordering budget cannot censor an independent zero-dep suffix.

## 8. Cross-authority operations

The universal order is:

```text
optional bounded permit -> TxPool -> PrePoolKernel
```

No kernel guard spans worker computation, external I/O or await. RPC/query,
administrative remove, clear, chain commit and local/worker handoff use shared
operations in `service/pipeline_ops.rs` so call sites cannot invent a different
structure list or reverse lock edge.

Administrative removal computes the accepted descendant closure while holding
`TxPool`, moves every pre-pool consumer of removed parents to exact
`Wait(Missing)`, removes the accepted closure, then publishes released-input
availability before the write guard opens.

Ordinary acceptance follows one paired boundary:

```text
capacity hint (owns no state) -> TxPool write -> Pool Plan
  -> PrePoolKernel handoff/settlement -> EffectJournal exact/static predicate
  -> total Pool Apply + append -> unlock
```

Every transaction-shaped, policy, conflict, liveness, arithmetic and capacity
error occurs before either accepted membership or its effects change. Internal
kernel defect containment and generation rebuilding remain P4 work; they are
not reclassified as transaction rejects.

## 9. Effects, chain changes and templates

Stable external effects are immutable, statically partitioned journal records
published outside authority locks. P3 deleted `EffectOutbox`, reservation IDs,
credit-across-lock ownership and journal-triggered service fail-stop. Remote
cannot consume trusted/critical progress; queued and active batches remain
exactly charged and publisher replacement resumes the active cursor.

Reorg reconciliation still uses `recovery_lock` to serialize detached replay,
clear and persistence. Attached outputs and released accepted inputs advance
the same dependency epochs used by ordinary commits. Detached transaction
sorting and cache lookup use full/witness identity. P4 moves the retained
replay owner to `RecoveryRetained` and deletes the cross-actor lock.

Block assembler priority is unchanged:

- `update_full` and reset are mutually exclusive;
- a full rebuild has priority over optimistic proposal/transaction deltas;
- skipped optimistic generations remain dirty and are retried;
- detached candidate uncles conflicting with recovered proposal paths are
  filtered, and stale `Gap` entries are reconciled to the new proposal window.

Normal `get_block_template` mining, not a hand-authored proposal block, is the
required regression for recovered dependent transactions.

## 10. Failure domains at P3

Typed transaction, backpressure and stale outcomes do not stop the service.
Worker panics are contained by worker supervision, and conflict-history
saturation terminalizes only that history owner.

One declared migration boundary remains: `PrePool::mutate_required`,
poisoned-mutex recovery, `recovery_lock` and the service-wide authoritative
failure latch are P4 compatibility debt. P4 replaces them with retained chain
ownership plus the designed `DefectDomain` rebuild/generation swap. The current
critical effect region is isolated, while P4 completes the constant-size
`GenerationReset` fallback for oversized chain/admin detail.

No hostile or legal capacity input may be routed to those structural paths.
Any newly discovered reachable trigger is a phase blocker and must be fixed at
the authority/Plan boundary, not classified as an ordinary reject or wrapped
with another rollback layer.

## 11. Executable review contract

All test code is physically separate from production modules. Test-only seams
are enumerated in `test-layout-manifest.json`; the independent kernel audit in
`component/tests/pre_pool_seam.rs` recomputes every projection and resource
counter without calling production transition repair code.

The current P2 evidence includes:

- exact seven-state and typed-outcome reference-model tests;
- full primary-to-projection rebuild after deterministic and randomized public
  transitions;
- ABA, short-ID collision, source promotion and witness-cache identity;
- closed total/remote/per-peer/conflict accounting, replacement-payload charge,
  fair owner heads and active caps;
- exact canonical causal keys, sequential expanded-parent loss, pruned
  dependency epochs, active-lease parent loss and bounded maintenance;
- full conflict-history behavior, remote deadline and wakeup without service
  panic or capacity self-retry;
- post-mutation overlay-level cell/header availability, so a same-branch
  create-and-spend cannot falsely wake conflict history or overwrite its
  public rejection, and retained conflict history never projects as RPC
  `Pending`;
- real service/RPC/RBF/reorg/template/persistence regressions.
- a 96-seed accepted-graph differential proving sparse Plan/total Apply matches
  the stepwise CPFP/status reference and leaves every rejected state unchanged;
- role-separated resolver/checker tests, both reader/spender arrival orders,
  2,000-reader fanout, selected-set ordering, exact SCC/downstream isolation,
  dense-cycle work fallback and dep-budget suffix liveness;
- full-hash accepted lookup, proposal-slot collision and exact witness-cache
  identity regressions.

Run:

```text
python3 devtools/check_tx_pool_review_guide.py
python3 devtools/check_tx_pool_test_layout.py
python3 devtools/check_tx_pool_security_manifest.py
cargo nextest run -p ckb-tx-pool --features internal --no-fail-fast
cargo clippy -p ckb-tx-pool --all-targets --features internal -- -D warnings
```

Process tests must run through `make integration`; the generated 149-spec
universe intentionally includes mining, RPC, relay, compact block, sync/fork,
DAO and hardfork ingress outside `test/src/specs/tx_pool`.

## 12. Checkpoints

| Checkpoint/phase | State | Evidence |
|---|---|---|
| C1 | Complete (`02e648255`) | preserved correctness fixes and froze the audited redesign; no benchmark |
| P0 / C2 | Complete (`8596c6c5d`) | architecture contract, independent reference model, 152-finding bridge and 149-spec impact universe |
| P1 / C3 | Complete (`1d9e0cf5b`) | concrete seven-state kernel cut over; old coordinator/runtime/conflict owner deleted; production Rust is 18,957 raw lines versus C2's 24,236 (−5,279, tests and benchmark excluded); test Rust is reported separately at 12,745 versus 21,650 and benchmark remains 1,422; 204/204 internal nextest, zero-warning all-target clippy and all document gates are green; all 18 targeted process integrations passed through `make integration`, including normal-mining reorg, RBF status/history, relay, orphan, collision and dependency-order boundaries |
| P2 / C4 | Complete | immutable accepted `PoolMutationPlan`, full-hash primary index, causal-only graph, role-aware resolution and selected-set SCC ordering cut over; nested undo/journal rollback/cell-ref escape deleted; internal instrumentation also uses Plan/Apply while permissive child-first construction is test-only; tx-pool production Rust is 18,757 lines (−200 from C3), test Rust is separately 13,051 and benchmark remains 1,422; 209/209 internal-feature nextest, production/internal clippy, all document gates and 16 targeted process integrations pass |
| P3 / C5 | Complete | concrete static `EffectJournal`; generic outbox/reservation IDs and credit-across-lock paths deleted; Remote/ordinary/critical region lattice, exact queued+active charge, total Apply+append, publisher cursor restart, one bounded callback circuit and allocation-free relay retry/timeout isolation; tx-pool production Rust is 18,751 raw lines (−6 from C4), tests are separately 12,885 and benchmark remains 1,422; 210/210 internal-feature nextest, two clippy profiles, all document gates and 12 targeted process integrations pass |
| P4 / C6 | Pending | `RecoveryRetained`, v2 persistence, `DefectDomain`, exact assembler generation and deletion of `recovery_lock` |
| P5 / C7 | Pending | final correctness, static, source-size and review acceptance |
| P6 / C8 | Pending | complete 149-spec process acceptance and classification |
| P7 / C9 | Deferred | controlled C1/develop/final A/B performance acceptance |

Every completed phase ends with a whole-architecture review. A correction may
complete a frozen rule or delete accidental encoding; it may not add an owner,
state, rollback layer, reverse lock edge, unbounded scan or input-triggerable
service fail-stop.
