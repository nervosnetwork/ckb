# Maintaining the transaction pool

Use this page for configuration, caller completion, diagnosis and migration.
[Architecture](ARCHITECTURE.md) owns the design contracts and source map.

## Changing a rule safely

Start at the rule's producer, then follow its observations, committed changes
and completion boundary. This index identifies the enforcement to inspect;
the linked implementation and architecture pages remain authoritative.

| Invariant | Producer and enforcement | Regression evidence | Consequence of violating it |
|---|---|---|---|
| A decision retains its original positive, negative and complete-relation premises | [Graph](../src/authority/membership/graph.rs) records policy reads in its borrowed [Plan](../src/authority/store/plan.rs); merging rejects changed observations and Apply checks original identities | [Contracts](../src/authority/tests/contracts.rs): original owner, missing spender and late-reader cases | Stale work can act on a successor or miss a new dependent |
| Guards cover the whole change in a single lock order | [CommitLocks and acquire](../src/authority/store.rs) derive read support and sorted unique guards; [OwnerChanges](../src/authority/store/apply.rs) derives write support from the edits; debug assertions check order and complete edit consumption | [Concurrency](../src/authority/tests/concurrency.rs): lock-footprint order/write dominance, mixed guards and compatible commit overlap | Deadlock, an unprotected read or a skipped owner edit |
| Owners, projections, charges and notice obligations commit together | [Application](../src/authority/store/apply.rs) preflights before mutation; exhaustive `Shard::apply_edit` handles local projections; retired payloads outlive guards | [Concurrency](../src/authority/tests/concurrency.rs): preflight refusal, history retry and payload retirement | Partial membership, inconsistent indexes or a destructor under authority locks |
| Admission counts each affected existing owner once against the shared bound | [Membership](../src/authority/membership.rs) budgets the union of replacement families, capacity-trim families and late-producer descendants | [Membership](../src/authority/tests/membership.rs): exact 100, refusing 101 and overlapping families | Unbounded mutation work or incorrect capacity refusal |
| Cancellation releases the capability it owns without erasing committed effects | [Job and active permit](../src/authority/jobs.rs), [budget](../src/authority/budget.rs) and [outbox](../src/authority/notice.rs) own separate lifetimes; activation follows guard release | [Execution](../src/authority/tests/execution.rs) and [notice](../src/authority/tests/notice.rs): pause/stop refund, cancelled waiters and publisher failure | Leaked capacity, missing publication or a blocked caller |
| Chain success acknowledges reconciliation and required publication | [Builder](../src/service/builder.rs) transfers the sole chain receiver to its consumer; [controller](../src/service/controller.rs) waits for its reply independently of RPC readiness | [Lifecycle](../src/service/tests/lifecycle.rs): real builder startup, abandonment, stop and structural chain failure during replay; [lifecycle contract](architecture/EXECUTION.md) | Lost startup transitions or a success response ahead of required effects |
| A published template still describes its captured sources | [Driver](../src/authority/template.rs) validates lifecycle, selected owners and uncle receipt at publication; [BlockTemplate](../src/block_assembler/template.rs) owns its time lower bound | [Template driver](../src/authority/tests/template_driver.rs): same-tip clear, owner reentry and stale uncle preparation; [assembler](../src/block_assembler/tests/mod.rs): byte and time boundaries | Stale mining content or an invalid template timestamp |
| Chain detachment rechecks the canonical time conditions applicable to accepted work | [Canonical time verification](../../verification/src/transaction_verifier.rs) shares candidate selection with `transaction_depends_on_time`; [membership](../src/authority/membership.rs) retains that result and [chain](../src/authority/chain.rs) recovers affected descendants | [Canonical transactions](../../verification/src/tests/transaction_verifier.rs): since forms, maturity roles and error order; [chain](../src/authority/tests/chain.rs): admission-derived sensitivity and descendant recovery | A transaction can remain accepted after its time condition becomes invalid, or unrelated work is needlessly requeued |

For a replacement-policy change, begin in membership preparation. Identify every
fact used to choose victims and validate backing, including empty relations.
The read-set types preserve observations that were recorded; they cannot infer
an omitted business premise. Use the late-reader, stale-rejection and shared
mutation-budget cases to challenge the changed rule before inspecting lock code.

For a new shard projection, start with `Shard::apply_edit`, full-generation
replacement and lifecycle refresh. Its exhaustive destructuring forces the new
field to be considered, while tests must establish the correct update. For a
new projection, extend the independent `assert_state` oracle in
[state transitions](../src/authority/tests/state_transitions.rs), including
clear and lifecycle changes. If it retains more memory, check `owner_amount` in
[budget](../src/authority/budget.rs) and `accepted_transaction_charge_bytes` in
[residency](../src/authority/residency.rs).

For a template optional-content change, start with `fit_optional_content`:
selected proposals consume bytes before compatible uncles, and only selected proposals
participate in conflict filtering.

Resource ceilings and fallible reservations do not promise recovery from every
allocation failure. The committed mutation tail still uses Rust's abort-on-OOM
contract, described in [commit and concurrency](architecture/COMMIT.md).

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
| `min_fee_rate` / `min_rbf_rate` | 1,000 / 1,500 | Shannons per kilobyte; admission and replacement policy |
| `max_ancestors_count` | 1,000 | Ancestor limit; legacy parsing floors smaller values to 1,000 |
| `expiry_hours` | 12 | Ordinary transaction expiration; replacement history has separate lifetime rules |
| `keep_rejected_tx_hashes_days` / `keep_rejected_tx_hashes_count` | 7 / 10,000,000 | Recent-rejection retention |
| `persisted_data` / `recent_reject` | Under the node's `tx-pool` data directory | Persistence file base and recent-rejection database; relative paths use the node root |

VM execution rate and startup allowance are calibrated internally and logged at
startup. Only network work has a time budget, capped at `MIN_BLOCK_INTERVAL`
(currently 8 seconds); see [verification budgets](architecture/EXECUTION.md#verification-budgets-and-proof-reuse).

`max_tx_pool_size` retains its existing serialized-byte meaning. Internal
[ResidencyLimits](../src/constants.rs) provide 1,000,000,000 accepted bytes and
384,000,000 pipeline bytes for serialized capacities up to 180,000,000 bytes.
Larger capacities scale both ceilings proportionally, rounding down to bytes.
Small pools still need room to resolve dependencies and execute transactions;
lowering serialized capacity therefore does not lower these internal ceilings.
Pipeline capacity includes both retained owners and reserved active-job envelopes.
These are charged limits, not preallocated memory or a whole-process RSS bound.
They are internal policy, so operators do not configure competing memory knobs.

An explicit `max_tx_verify_workers = 0` starts no background verification workers
and emits a warning. Network and test submissions remain queued; synchronous
local submission, chain control and templates still run.

`Limits::new` rejects unusable envelopes, zero ancestors and arithmetic overflow.
Construction requires a multi-thread Tokio runtime. Increasing workers divides
the active byte/edge envelope among more jobs and can make a previously viable configuration invalid.
Check the full calculation in [budget.rs](../src/authority/budget.rs) before tuning.

## Caller completion

The [controller](../src/service/controller.rs) owns the exact return types.
Synchronous APIs use the service runtime; direct callback mutation is rejected.

| Operation | Successful return establishes |
|---|---|
| `submit_local_tx` | Completed verification/admission outcome; inspect both transport result and inner `Reject` result |
| `submit_local_test_tx` | Local resolution completed and script verification queued; missing or spent dependencies reject before queueing |
| `test_accept_tx` | Same admission policy and final validation without insertion or relay |
| `remove_local_tx` | Removes the selected transaction and accepted descendants; preserves the network's recent-known history |
| `submit_remote_tx` / `submit_remote_txs` | Ingress processing, not accepted membership; batch outcome names the processed input prefix, including rejects |
| `notify_txs` / `notify_new_uncle` | Admission to the bounded request route, not completion of downstream work |
| `update_tx_pool_for_reorg` | Reliable reconciliation and required publication completed |
| `update_ibd_state` | Fee-estimator IBD state updated on the ordered chain route, independent of suspended transaction handlers |
| `get_block_template` | Validated mining template returned within one 30-second request deadline, including queueing; timeout does not cancel the shared builder |
| `clear_pool` / `clear_verify_queue` | Reliable administrative request completed |
| `stop` | Cancellation signalled; tasks can still be joining |
| `suspend_chunk_process` / `continue_chunk_process` | Pool computation pause/resume requested; suspension is cooperative and does not establish VM quiescence |
| `service_started` | Startup replay has completed when true; false acknowledges neither closed request queues nor completed shutdown joins |

## Operational triage

Use existing [metrics](../src/metrics.rs) and named failure logs before adding
instrumentation. Gauges are observations, not admission authority.

| Symptom | Inspect first | Next step |
|---|---|---|
| Remote pressure while local progress remains possible | `ckb_tx_pool_pipeline_residency`: remote/total entries and bytes, active work | Distinguish retained backlog from active compute; check per-peer limits and workload shape |
| Commits or reorg calls wait after state changed | `ckb_tx_pool_effect_usage`: batches/bytes, publisher logs | Identify an unready FIFO head, slow synchronous endpoint or endpoint failure |
| VM-time rejection | Rejection class and startup calibration values | Reproduce actual VM work; do not classify local resource refusal as consensus invalidity |
| Template refresh error | Selected-owner/lifecycle source and template-driver logs | Check invalidation or build failure; a refresh deadline does not cancel the shared driver |
| Shutdown stalls | Handler/background join versus publisher join | Follow owned tasks and synchronous providers; `started=false` is insufficient |
| Startup has no restored transactions | Persistence-load log and v2 file | Preserve the failed input for diagnosis; malformed v2 does not fall back to v1 |

Follow [execution and lifecycle](architecture/EXECUTION.md) for scheduling,
publication and shutdown ownership, or [profiling](PROFILING.md) for CPU and task
observations.

## Repeat composed workloads and replay sequences

The normal suite runs two rounds of the [mixed service workload](../src/authority/tests/stability.rs)
and 16 seeds of 512 [legal state operations](../src/authority/tests/state_sequences.rs).
Extended runs are ignored by default. From the repository root:

```sh
RUST_BACKTRACE=1 cargo nextest run --locked -p ckb-tx-pool --lib \
  --test-threads 1 --run-ignored only --success-output final \
  -E 'test(extended_mixed_load_stability)'

cargo nextest run --locked -p ckb-tx-pool --lib \
  --test-threads 1 --run-ignored only --success-output final \
  -E 'test(extended_state_sequences)'
```

The extended mixed workload runs 4,096 rounds in one service generation. Each
round combines remote pressure, replacement, dependency wakeup, attach/detach,
recovery, same-peer retry and clear. It checks exact callback/relay outcomes,
owned-allocation release and empty owner/index/queue/account/active/outbox
populations. It uses canonical transaction verification and database live cells,
but supplies block snapshots without node consensus, network relay or mining.

| Variable | Effect |
|---|---|
| `TX_POOL_STABILITY_ROUNDS` | Select 1–65,536 rounds before running |
| `TX_POOL_STABILITY_MEMORY` | Report current and lifetime peak RSS on macOS/Linux at readiness, at most 64 released-round checkpoints and joined shutdown |
| `TX_POOL_STABILITY_TRACE` | Write prefixed observations to a new file while Nextest captures stdout; existing files are rejected |
| `TX_POOL_SEQUENCE_SEED` | First seed for the extended sequence run; subsequent seeds wrap consecutively |
| `TX_POOL_SEQUENCE_REPLAY` | Replay a saved failure JSON instead of generating commands |

`TX_POOL_STABILITY_RESULT` retains individual latencies and p50/p95/p99/max for
local parent responses, chain attach/detach, recovery completion and clear.
Local/attach latency includes a controlled callback hold. Logical release does
not require RSS to return to startup: verification/database caches and allocator
retention remain. These instrumented observations do not replace an
[uninstrumented comparison](BENCHMARK.md).

The extended sequence run covers 64 seeds of 4,096 steps. It calls real planners
and Store over three transaction families, checking complete populations and
stale-capture rejection after each step. Verified cells/cycles and snapshots are
fixtures; publication is silenced. This tests state transitions, while the mixed
workload tests service execution and lifetimes.

On failure, save the JSON from `TX_POOL_SEQUENCE_FAILURE` as
`/tmp/tx-pool-sequence.json`. It contains the seed, original and reduced commands,
and failure details. Replay it with:

```sh
TX_POOL_SEQUENCE_REPLAY=/tmp/tx-pool-sequence.json \
  cargo nextest run --locked -p ckb-tx-pool --lib \
  --test-threads 1 --run-ignored only --success-output final \
  -E 'test(extended_state_sequences)'
```

Replay schema 1 contains `commands`: `[kind, transaction, origin]` triples.
Kinds 0–7 are Receive, Resolve, Admit, Remove, Expire, Clear, Attach and Detach.
Receive origins 0–5 are local, recovery, proposal and three remote fixtures;
origin 6 promotes an existing remote owner to proposal. Transaction IDs are 0–11;
Clear uses its second value as the pipeline-only boolean. Unused fields must be
zero. Unknown encodings, files over 8 MiB, more than 65,536 commands and missing
prerequisites are rejected.

Reduction preserves the failing command and named invariant, or the full panic
message for unlabelled failures. Invalid sequences are not reproductions. The
result allows no single-command deletion under that criterion; it is not a
shortest-trace or exhaustive-coverage claim. Retain the original trace, complete
output, source, toolchain, features and host identity.

## Add a rejection reason

Define malformed status, recent-record eligibility (`should_recorded`) and relay
eligibility in [Reject](../../util/types/src/core/tx_pool.rs). Its top-level matches
are exhaustive; dynamic verification/resolution errors still use payload-based
classification. A local resource refusal must not revoke a peer cohort.

Update [RPC errors](../../rpc/src/error.rs),
[public serialization](../../util/jsonrpc-types/src/pool.rs),
[metrics](../src/metrics.rs) and [notice diagnostics](../src/authority/notice.rs)
as their exhaustive matches require. Policy is captured before diagnostic
strings are bounded. Recent records answer status queries; they do not gate
admission. Terminal capacity rejection, including eviction, remains queryable;
ingress capacity refusal only diagnoses pressure and releases relay tracking.
VM timeouts and node interruptions leave verification unfinished. They release
network retry tracking without becoming recent records or rejection events.
`PoolTransactionReject::try_from` excludes these outcomes;
adding an internal outcome does not require a new public rejection value.

Regenerate RPC documentation with `make gen-rpc-doc` and check it with
`make check-dirty-rpc-doc`. Extend the Reject policy matrix and affected ingress
and notice tests; preserve policy and public error shape through detachment.

## Integrating callers and stored data

The Rust API has intentional source changes requiring a SemVer-major release
relative to the prior published crate. The current package remains Unreleased;
versioning is a release action. The old exported TxPool is gone;
use controller operations. Builder construction is fallible and returns the
builder, controller and sole `TxVerificationResultReceiver`, rather than taking
an external relay sender. Registered callbacks receive `TxEntrySnapshot`; reject
callbacks no longer receive mutable pool access. See [builder](../src/service/builder.rs),
[callback types](../src/callback.rs) and [shared assembly](../../shared/src/shared_builder.rs).
`update_tx_pool_for_reorg` takes detached blocks, attached blocks and the successor
snapshot; remove the old detached-proposal-ID argument from callers. Proposal
projection comes from the paired snapshots.

Controller replies keep operation failures distinct from transaction rejections.
Local submission still returns an inner `Reject` for admission policy, while
service failures use the outer `AnyError`; they do not become channel receive
errors. Replying to a caller, including one that has cancelled, does not suppress
a structural fault's escalation to the service supervisor.

Released node configurations retain the units and conversion rules listed
[above](#configuration-and-capacity). Obsolete `max_mem_size`, `max_cycles` and
three cache-size fields remain accepted and ignored. Missing verification
settings receive defaults; explicit `arrival_time` remains supported. Unreleased
intermediate configuration keys have no compatibility guarantee.

[Persistence](../src/persisted.rs) reads v1/v2, preferring v2 when present, and
writes v2 accepted/recovery partitions through a temporary file and rename.
After committing v2, the writer removes the superseded v1 migration file.
The accepted partition retains resolved parent and read-before-spend order across
save and load, independently of proposal phase. Conditional cycles retain every
body in causal order for fresh admission. Recovery bodies follow the accepted
prefix and are ordered by their raw dependencies. Replay verifies every body;
it does not restore prior acceptance proof or replacement blockers.
Back up persisted data before upgrade. Downgrade
and reverse conversion are unsupported. Invalid v2 input is logged and startup
continues with an empty replay set; v1 is only considered when v2 is absent.
Merging and dependency ordering also finish before workers start, so a returned
preparation error can discard recovery without faulting the live pool. The file
read bound does not bound all expanded allocations or prevent process-wide OOM.
The writer syncs the temporary file before rename, but does not fsync its parent
directory; do not infer universal crash durability. [Persistence tests](../src/tests/persisted.rs)
cover legacy loading, partition/order round-trip and bounded reads.

[RemoteTxBatchOutcome](../src/service/message.rs) identifies the processed input
prefix. The [relayer](../../sync/src/relayer/transactions_process.rs) releases known
marks for the remaining suffix, including on cancellation. Its sole result
consumer drains committed observations synchronously before async network sends.
[Reset and reconstruction](architecture/EXECUTION.md#waiting-chain-and-recovery)
restore waiting-parent requests, not all accepted results. Mailbox delivery does
not guarantee network delivery.

The `ckb_relay_tx_verify_result_queue_size` metric observes this mailbox, with
its item bound exposed as `ckb_relay_tx_verify_result_queue_capacity`. Mailbox
draining continues without peers and during IBD; the metric does not represent
the relayer's separate pending-broadcast cache or remaining reconstruction work.

`ckb_relay_tx_verify_result_queue_resets{reason="capacity"|"accounting"}`
counts forced mailbox reconstruction. An ordinary authority `GenerationReset`
does not increment it unless the mailbox also overflows. The accounting reason
indicates an internal projection inconsistency, not normal pressure.

`ckb_relay_pending_transactions_discarded{reason="capacity"|"reset"}` counts
announcements evicted by the relayer cache or discarded when its projection is
reset. Updating an existing announcement, draining it for a broadcast attempt
or removing a rejected transaction does not count as capacity loss. A reset can
originate from authority policy or mailbox reconstruction; these counters do not
establish whether a peer received a transaction.

Transaction subscription handoffs already report full/closed drops through
`ckb_notify_transaction_dropped{boundary,reason}`. Verification interruptions
remain internal retryable outcomes; their absence from public rejection events
is the contract described under [rejection reasons](#add-a-rejection-reason).
