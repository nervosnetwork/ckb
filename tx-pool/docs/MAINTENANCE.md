# Maintaining transaction-pool behavior

The [controller](../src/service/controller.rs) defines caller completion and failure.
[ARCHITECTURE.md](ARCHITECTURE.md) explains ownership and locks.
[Store::apply](../src/authority/store.rs) validates and commits each prepared Plan.

## Configuration and capacity

Values below are current defaults from [legacy conversion](../../util/app-config/src/legacy/tx_pool.rs)
and [TxPoolConfig](../../util/app-config/src/configs/tx_pool.rs). Byte values are
bytes, not MiB. Parsed legacy files and programmatically constructed configs
share validation, but legacy ancestor normalization has its own compatibility floor.

| Setting | Default | Meaning |
|---|---:|---|
| `max_tx_pool_size` | 180,000,000 | Accepted serialized transaction bytes |
| `max_tx_verify_workers` | `max(3 × CPU cores / 4, 1)` | Worker population; compute permits also depend on Tokio runtime size |
| `verify_ordering` | `fee_rate` | `arrival_time` or `fee_rate`; selection order, not completion order |
| `max_tx_verify_cycles` | `TWO_IN_TWO_OUT_CYCLES × 20` | Remote small/large scheduling boundary, not a VM limit |
| `min_tx_verify_time_ms` / `max_tx_verify_time_ms` | 250 / 8,000 | Cumulative active VM-work time, including initial loading |
| `tx_verify_cycles_per_ms` | 10,000 | Local signal for choosing that time budget; not consensus accounting |
| `max_tx_verify_initial_load_bytes` | 268,435,456 | Cumulative bytes mapped loading a root program |
| `min_fee_rate` / `min_rbf_rate` | 1,000 / 1,500 | Shannons per kilobyte; admission and replacement policy |
| `max_ancestors_count` | 1,000 | Ancestor limit; legacy parsing floors smaller values to 1,000 |
| `expiry_hours` | 12 | Ordinary transaction expiration; replacement history has separate lifetime rules |
| `keep_rejected_tx_hashes_days` / `keep_rejected_tx_hashes_count` | 7 / 10,000,000 | Recent-rejection retention |
| `persisted_data` / `recent_reject` | Under the node's `tx-pool` data directory | Persistence file base and recent-rejection database; relative paths use the node root |

`max_tx_pool_size` retains its existing serialized-byte meaning. Internal
[ResidencyLimits](../src/constants.rs) provide 1,000,000,000 accepted bytes and
384,000,000 pipeline bytes for serialized capacities up to 180,000,000 bytes.
Larger capacities scale both ceilings proportionally, rounding down to bytes.
Small pools still need room to resolve dependencies and execute transactions;
lowering serialized capacity therefore does not lower these internal ceilings.
Pipeline capacity includes both retained owners and reserved active-job envelopes.
These are charged limits, not preallocated memory or a whole-process RSS bound.
They are internal policy, so operators do not configure competing memory knobs.

`Limits::new` rejects unusable envelopes, zero ancestors, zero cycle-rate/load/time
bounds, inverted time bounds and arithmetic overflow. Construction requires a
multi-thread Tokio runtime. Increasing workers divides the active byte/edge
envelope among more jobs and can make a previously viable configuration invalid.
Check the full calculation in [budget.rs](../src/authority/budget.rs) before tuning.

## Caller completion

The [controller](../src/service/controller.rs) owns the exact return types.
Synchronous APIs use the service runtime; direct callback mutation is rejected.

| Operation | Successful return establishes |
|---|---|
| `submit_local_tx` | Completed verification/admission outcome; inspect both transport result and inner `Reject` result |
| `test_accept_tx` | Same admission policy and final validation without insertion or relay |
| `submit_remote_tx` / `submit_remote_txs` | Ingress processing, not accepted membership; batch outcome names the processed input prefix, including rejects |
| `notify_txs` / `notify_new_uncle` | Admission to the bounded request route, not completion of downstream work |
| `update_tx_pool_for_reorg` | Reliable reconciliation and required publication completed |
| `clear_pool` / `clear_verify_queue` | Reliable administrative request completed |
| `stop` | Cancellation signalled; tasks can still be joining |
| `suspend_chunk_process` / `continue_chunk_process` | Pool computation pause/resume requested; suspension is cooperative and does not establish VM quiescence |
| `service_started` | Startup replay has completed when true; false does not prove shutdown joins finished |

## Operational triage

Use existing [metrics](../src/metrics.rs) and named failure logs before adding
instrumentation. Gauges are observations, not admission authority.

| Symptom | Inspect first | Next step |
|---|---|---|
| Remote pressure while local progress remains possible | `ckb_tx_pool_pipeline_residency`: remote/total entries and bytes, active work | Distinguish retained backlog from active compute; check per-peer limits and workload shape |
| Commits or reorg calls wait after state changed | `ckb_tx_pool_effect_usage`: batches/bytes, publisher logs | Identify an unready FIFO head, slow synchronous endpoint or endpoint failure |
| VM-time rejection | Rejection class and configured time/load envelopes | Reproduce actual VM work; do not classify local resource refusal as consensus invalidity |
| Template refresh error | Selected-owner/lifecycle source and template-driver logs | Check invalidation or build failure; a refresh deadline does not cancel the shared driver |
| Shutdown stalls | Handler/background join versus publisher join | Follow owned tasks and synchronous providers; `started=false` is insufficient |
| Startup has no restored transactions | Persistence-load log and v2 file | Preserve the failed input for diagnosis; malformed v2 does not fall back to v1 |

For CPU or scheduling attribution, follow [profiling](PROFILING.md). Raising
budgets or timeouts should follow a diagnosed capacity requirement.

## Change admission or replacement policy

Start at `TxPoolController::submit_local_tx`, `submit_remote_txs`, or
`test_accept_tx`. [Direct submissions](../src/authority/service/submission.rs) and completed
[verification jobs](../src/authority/service/execution.rs) reach the same
[membership::admission](../src/authority/membership.rs).
That module owns membership, RBF, ancestor and eviction decisions. Transport
bounds remain in ingress. Jobs own canonical resolution, original observations
and settlement; [verification adapters](../src/verification.rs) apply the canonical
checks and node-local execution budgets.

Change `rbf` or `prepare_admission` for the relevant rule and capture every new
producer, spender or closure premise through Graph's original ReadSet. Return
owner edits and effects in the existing Plan; do not mutate an incumbent while
deciding. Preserve checked fee arithmetic and count shared descendants once.
Capacity trimming can enlarge the victim set, so the later backing validation
has a different input from the earlier check. Dry-run uses this policy and final
validation without publishing effects or releasing incumbent ownership.

In [membership tests](../src/authority/tests/membership.rs), start with
`replacement_counts_shared_descendants_once_and_accepts_the_exact_fee_boundary`,
`stale_replacement_rejection_rechecks_the_conflict_that_caused_it`, and
`dry_run_uses_the_same_policy_without_mutating_membership_or_releasing_victim_charge`.

## Adjust a resource or scheduling limit

Configuration enters through `TxPoolServiceBuilder::new` and
[TxPoolConfig](../../util/app-config/src/configs/tx_pool.rs).
[ResidencyLimits](../src/constants.rs) derives accepted and pipeline byte ceilings
from serialized capacity; [Limits::new](../src/authority/budget.rs) divides these
into resource envelopes once. Reorg payload and persistence-read bounds use the
same derivation. Changing its policy requires reviewing all three consumers;
the same module's owner charges and OwnerDelta govern reservation and release.
Representation changes also update the [residency calculation](../src/authority/residency.rs).
Serialized accepted size, retained bytes, edges and active work are distinct limits.
Check rejected reservation, stale Apply, cancellation and successful retirement;
all must return the same owned capability exactly once and preserve trusted room.

For scheduling, `max_tx_verify_cycles` separates remote declared-cycle lanes.
[Queues](../src/authority/queue.rs) derives both Resolve and Verify lane choices
from immutable Source and its one threshold. The threshold is neither a
VM/admission limit nor a block-priority exemption: all lanes and local requests
share the [computation gate](architecture/EXECUTION.md#scheduling-and-block-priority).
Preserve `arrival_time`/`fee_rate` selection order, without promising completion
order or storing another classification in Resolved.
In [legacy conversion](../../util/app-config/src/legacy/tx_pool.rs), the default
ancestor count and compatibility floor have separate names. Both are currently
1000; public Default explicitly selects the default after conversion, while parsed
legacy values retain the compatibility floor.

Use [budget tests](../src/authority/tests/budget.rs), especially
`positive_reservation_rolls_back_when_dropped_and_exact_owner_charge_is_released`;
`serialized_capacity_keeps_its_units_and_small_pools_keep_execution_room` checks
small and larger pools without changing the old serialized-size contract.
[execution tests](../src/authority/tests/execution.rs) cover unusable configuration
and trusted progress under notice pressure. The [queue boundary test](../src/authority/tests/queue.rs)
`both_phases_use_the_same_declared_cycle_boundary` covers both ordering settings.

## Add a rejection reason

Local submit and dry-run return Reject; [remote ingress](../src/authority/ingress.rs)
consumes its policy and publishes notices. Define the three policy decisions in
[Reject](../../util/types/src/core/tx_pool.rs): malformed status, recent-record
eligibility (`should_recorded`) and relay eligibility. The top-level matches
are exhaustive, so a new variant must make each decision explicitly.
Dynamic verification and resolution errors still require their payload-based
classification. Malformed declared remote work can revoke its current peer
cohort; a transient local resource failure must not accidentally acquire that effect.

Update [RPC error mapping](../../rpc/src/error.rs),
[public rejection serialization](../../util/jsonrpc-types/src/pool.rs),
[metrics classification](../src/metrics.rs) and
[notice diagnostic detachment](../src/authority/notice.rs) as required by their
exhaustive matches. Policy is captured before diagnostic detachment; bounded
strings must preserve the selected behavior. Recent records answer status queries
and do not gate admission. Regenerate RPC documentation with `make gen-rpc-doc`
and verify it with `make check-dirty-rpc-doc`; do not edit generated output by hand.
Extend the Reject policy matrix, the
[atomic peer-revocation tests](../src/authority/tests/ingress_contracts.rs), and
`rejection_diagnostics_keep_policy_public_shape_and_bounded_owned_strings` in
[notice tests](../src/authority/tests/notice.rs).

## Diagnose reorg progress or invalidation

Start at `TxPoolController::update_tx_pool_for_reorg`; the
[chain caller](../../chain/src/verify.rs) supplies the successor snapshot.
The reliable bounded chain lane applies backpressure and waits for reconciliation
and publication. It remains usable during startup and while block verification
pauses pool computation. A growing verification queue during a block backlog is
expected; its owners and suspended VM state remain retained. Once the backlog
empties, the block consumer signals Resume before waiting for more blocks.
The compatibility argument containing detached proposal IDs does not decide
projection; paired snapshots supply those facts.

[Pool::reconcile](../src/authority/service.rs) owns the ChainPause through commit
and publication. [chain::reconcile](../src/authority/chain.rs) prepares the owner
and snapshot changes; Store validates their original observations. Inspect the
attached tip, paired view and invalidated owner identities before changing retry
behavior. Planner or commit capacity refusal selects bounded recovery; an
oversized command uses exact-snapshot generation replacement. A changed query
snapshot with an unfinished reorg call can mean publication is still waiting.
Successful bounded recovery logs its resource refusal; this does not imply every
candidate was retained. Ordinary reconciliation computes graph aggregates only
when a rejection or status-change callback needs them.

Detached uncles are optional post-commit template input. Their retained-payload
ceiling belongs to `BoundedCandidateUncle::payload_limit` in
[candidate_uncles.rs](../src/block_assembler/candidate_uncles.rs), shared with startup.
Use [chain tests](../src/authority/tests/chain.rs) for stale-plan atomicity,
mismatched attached tips and bounded parent-first recovery, and
[controller tests](../src/service/tests/controller.rs) for reliable startup delivery,
oversized snapshot replacement and clear/reorg ordering.

## Diagnose shutdown or publication progress

`TxPoolController::stop` signals cancellation; it does not join the service.
[TxPoolServiceBuilder::run](../src/service/builder.rs) owns the shutdown sequence:
stop intake and workers, close request queues, join handlers/readers/background
tasks while the publisher remains alive, then close and join the outbox publisher.
Only eligible, fully drained generations reach persistence. Preserve that
ownership sequence when adding a task; an aborted future must still be joined.

In [Outbox](../src/authority/notice.rs), distinguish an unready FIFO head from a
running endpoint and from a full outbox. A selected ready prefix remains bounded;
each batch settles, returns capacity and releases FIFO/publisher references
before the next. Waiting callers and callback-owned clones have their own lifetimes.
Callbacks run without Store guards. Callback panic disables all callback kinds
for that publisher; other endpoints can continue. A synchronous callback must
return before its batch and publisher can finish: timeout/abort cannot stop it.
Read callbacks have a reserved route; direct mutating reentry fails. Do not join
a helper thread that synchronously reenters mutation, since its TLS marker differs.

Use `publisher_abort_keeps_a_running_callback_and_its_batch_owned_until_return`
and `later_callback_observes_prior_batch_settled_and_released` in notice tests;
execution tests cover callback reads, service join and preserving the prior
persistence file after a fault. Use [observable ordering](REVIEW_GUIDE.md#representative-checks), not elapsed time alone.

## Integrating callers and stored data

The Rust API has intentional source changes requiring a SemVer-major release
relative to the prior published crate. The current package remains Unreleased;
versioning is a release action. The old exported TxPool is gone;
use controller operations. Builder construction is fallible and returns the
builder, controller and sole `TxVerificationResultReceiver`, rather than taking
an external relay sender. Registered callbacks receive `TxEntrySnapshot`; reject
callbacks no longer receive mutable pool access. See [builder](../src/service/builder.rs),
[callback types](../src/callback.rs) and [shared assembly](../../shared/src/shared_builder.rs).

Configuration compatibility covers upgrades from released node configuration
files. Existing sizes, fees, worker counts, retention and paths retain their
units and conversion rules, including the historical ancestor floor. Obsolete
`max_mem_size`, `max_cycles` and the three cache-size fields remain accepted and
ignored. Missing new verification settings receive defaults, including
`verify_ordering = "fee_rate"`; explicit `arrival_time` remains supported.
Unreleased intermediate configuration keys have no compatibility guarantee.
Check [conversion tests](../../util/app-config/src/legacy/tx_pool.rs) and bundled
network-config parsing. Scheduling thresholds and VM-work budgets are local policy.

[Persistence](../src/persisted.rs) reads v1/v2, preferring v2 when present, and
writes v2 accepted/recovery partitions through a temporary file and rename.
Replay verifies transaction bodies again; it does not restore prior acceptance
proof or replacement blockers. Back up persisted data before upgrade. Downgrade
and reverse conversion are unsupported. Invalid v2 input is logged and startup
continues with an empty replay set; v1 is only considered when v2 is absent.
Merging and dependency ordering also finish before workers start, so a returned
preparation error can discard recovery without faulting the live pool. The file
read bound does not bound all expanded allocations or prevent process-wide OOM.
The writer syncs the temporary file before rename, but does not fsync its parent
directory; do not infer universal crash durability. [Persistence tests](../src/tests/persisted.rs)
cover legacy loading, partition/order round-trip and bounded reads.

Remote submission completion describes ingress processing, not final verification.
[RemoteTxBatchOutcome](../src/service/message.rs) identifies the processed input prefix;
the [relayer](../../sync/src/relayer/transactions_process.rs) releases known marks
for the suffix without a successful ingress outcome, including on cancellation.
Its sole result consumer drains bounded committed observations. GenerationReset
clears sync's known and pending relay state; after the mailbox drains, RelayDrain
rebuilds UnknownParents for current waiting remote owners in bounded pages.
It does not replay all accepted transactions. Later Ok results restore their own
known and pending entries. Preserve [synchronous result consumption](../../sync/src/relayer/mod.rs)
before async network sends; mailbox delivery is not a network-delivery guarantee.
