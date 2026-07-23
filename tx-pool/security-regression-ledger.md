# Tx-Pool Security and Regression Ledger

This ledger is the migration gate for the tx-pool pipeline refactor. It is
derived from the historical review notes in the workspace, the validated
security reports, and the reorg/template regression. The source notes remain
unchanged; this tracked document records what the implementation must preserve.

Status meanings:

- **Covered**: a focused automated regression exists.
- **Model-covered**: the invariant has a focused regression in the isolated
  target model, but production cutover and differential evidence are still
  required before deleting the legacy owner.
- **Partial**: behavior is covered indirectly or only one side of the invariant
  is locked; the listed follow-up is mandatory before deleting the legacy path.
- **Open**: no sufficient automated regression exists yet.
- **Accepted**: intentional compatibility behavior, with its resource/safety
  boundary stated explicitly.
- **Obsolete in target**: the legacy mechanism will be removed, but its security
  property must be covered at the replacement boundary.

## Architectural invariants

| ID | Invariant | Required evidence |
|---|---|---|
| I1 | One lifecycle owner | Before acceptance, a transaction payload has exactly one owner, one typed location and one revision in PipelineCoordinator; dependency/conflict/queue/deadline structures are derived ID indexes. Acceptance consumes that record and transfers sole authority to TxPool, which is never shadowed. |
| I2 | Authoritative commit | Only the transaction sequenced by the existing TxPool write guard can accept/reject RBF or mutate pool membership; coordinator finalization completes before that guard opens. |
| I3 | Transactional rollback | Failed replacement restores every removed entry before releasing competing candidates. |
| I4 | No silent loss | Every admitted transaction reaches pool, wait state, explicit rejection, or retryable internal failure. |
| I5 | Bounded untrusted state | Global and per-peer count/byte/active-work limits continuously cover queued, active and parked payloads, conservative dependency/conflict/ticket/deadline metadata, and any terminal payload charge retained by the bounded effect outbox. |
| I6 | Event-driven dependencies | Parent commit/failure wakes or reclassifies children; no child relies solely on polling or expiry. |
| I7 | Reliable chain transitions | Reorg deltas are ordered, tip-checked, retained until success, and never best-effort. |
| I8 | Stable-state effects | Callbacks, relay events, metrics, and notifications are appended to a bounded sequence-ordered outbox before the pool write guard opens, then run after internal ownership is stable and outside locks. |
| I9 | Deterministic conflict scheduling | Fee preference may order work, but speculative state cannot create a terminal verdict or fee-floor bypass. |
| I10 | Pool graph integrity | Entries, links, accounting, conflict closure, ancestor/descendant weights, and eviction journals change atomically. |
| I11 | Template liveness | Pool status, proposal selection, uncles, byte accounting, and template revisions cannot strand a valid transaction. |
| I12 | Performance non-regression | The target passes the checkpoint A/B throughput, latency, CPU, allocation, RSS, reorg, and template gates. |

## Historical findings

| # | Status | Regression anchor / required follow-up | Target invariant |
|---:|---|---|---|
| 1 | Covered | `escape_hatch_eviction_drops_cascaded_parents_from_parent_set` | I3, I10 |
| 2 | Covered | `reorg_retain_duplicate_does_not_cascade_dependents` | I4, I7 |
| 3 | Covered | `worker_exits_when_command_channel_dropped` | I4 |
| 4 | Covered | `superseded_candidate_is_not_double_parked_by_after_process` | I1, I9 |
| 5 | Covered | `winner_committing_without_replacement_restores_displaced` | I2, I9 |
| 6 | Covered | `find_winner_returns_strongest_with_fee_rate` | I9 |
| 7 | Partial | Recording predicate is tested; add end-to-end queue-full resubmission coverage. | I4 |
| 8 | Covered | `remote_duplicate_is_not_relayed_as_reject` | I4, I8 |
| 9 | Covered | `malformed_remote_preflight_bans_peer_and_records_reject` | I4, I8 |
| 10 | Partial | RBF tests traverse the path; target model test must make conflict snapshot/registration atomic. | I2, I9 |
| 11 | Covered | RBF integration family plus recent-reject predicate tests. | I4, I9 |
| 12 | Covered | `pre_check_worker_notifies_relayer_when_ordered_resolve_queue_is_full` | I4 |
| 13 | Covered | `recover_gives_up_terminally_after_bounded_retries` | I4 |
| 14 | Covered | `local_orphan_with_stuck_parent_is_eventually_rejected` | I4, I6 |
| 15 | Partial | The split prototype covers ready-parent + downstream-full wake-up, but the reviewed coordinator removes internal payload queue `Full`: a residency-charged entry changes an ID-only stage ticket atomically. Port a differential test proving stage handoff cannot lose the transaction. | I4, I6 |
| 16 | Partial | Worker panic guards and callback-panic containment are tested; add injected panic from resolve/verify/commit through final relay and ownership cleanup. | I1, I4 |
| 17 | Covered | `remove_tx_reports_in_progress_for_worker_active_job` | I1 |
| 18 | Covered | `banned_peer_job_is_dropped_by_pre_check_worker` | I4, I5 |
| 19 | Covered | `remove_tx_clears_double_parked_transaction_from_both_rooms` | I1 |
| 20 | Covered | Successful and failed replacement cascade tests. | I2, I3 |
| 21 | Partial | Reconcile return contract is tested; add reconcile-to-conflict-scheduler cleanup end to end. | I1, I7 |
| 22 | Covered | `attached_winner_finalizes_held_candidates` | I7, I9 |
| 23 | Covered | `reorg_attached_orphan_is_removed_without_dead_rejection` | I4, I7 |
| 24 | Covered | `clear_resets_expiry_watermark` | I1 |
| 25 | Covered | `wake_by_winner_keeps_other_reasons_and_stats_intact` | I1 |
| 26 | Accepted | Conflict audit entries intentionally persist; count/byte budgets are mandatory and target separates audit from executable state. | I1, I5 |
| 27 | Covered | `parent_added_after_child_gets_descendant_weight` | I10 |
| 28 | Covered | `conflict_closure_ignores_ghost_link_nodes` | I10 |
| 29 | Covered | `remove_expired_cascades_to_descendants` | I10 |
| 30 | Covered | On-chain reconcile includes cell deps. | I10 |
| 31 | Covered | `counter_drift_is_recovered_by_recompute` and saturating-counter tests. | I5, I10 |
| 32 | Covered | `failed_commit_restores_all_size_evictions_with_original_status_in_lock` asserts the one terminal `Full` event for the rejected candidate and no terminal event for entries restored from the eviction journal. | I4, I5 |
| 33 | Covered | `parent_parked_in_waiting_room_counts_as_in_flight` | I6 |
| 34 | Covered | `dropped_verify_mgr_cancels_its_worker_generation` | I1, I4 |
| 35 | Covered | `save_pool_waits_for_recovery_lock` | I7 |
| 36 | Partial | Same lock path is exercised; add explicit clear-during-recovery final-state test. | I7 |
| 37 | Covered | `full_and_uncle_updates_share_template_serialization_lock` | I11 |
| 38 | Partial | Submit acceptance callback lock test exists; add callback re-entry for reorg pending/proposed/reject batches. | I8 |
| 39 | Partial | Functional restore tests exist; keep a large-restore latency/contention benchmark. | I5, I12 |
| 40 | Covered | `cancel_during_backoff_exits_immediately` | I4 |
| 41 | Covered | `cancel_drains_deferred_recover_txs` | I4 |
| 42 | Partial | `failed_rbf_rollback_and_settlement_precede_deferred_publication` proves a saturated deferred channel cannot delay rollback or RBF ownership settlement; add saturated-channel throughput and shutdown stress. | I4, I5, I8, I12 |
| 43 | Covered | `zero_max_workers_is_clamped_to_one` | I4 |
| 44 | Covered | `dispatcher_channel_close_quiesces_workers_and_persists_pool` proves sender-drop shutdown cancels and joins workers, drains handlers, and persists accepted state before the dispatcher handle completes. | I4, I7 |
| 45 | Covered | `clear_pool_resets_template_and_notifies_miner_immediately` exercises clear → reliable Reset delivery → blank template → immediate miner notification. | I8, I11 |
| 46 | Partial | O(1) index is functional; add compact-block lookup scaling benchmark. | I12 |
| 47 | Covered | `budget_eviction_is_oldest_first` | I5 |
| 48 | Partial | Orphan recovery tests traverse batched lookup; add query-count or scaling benchmark. | I6, I12 |
| 49 | Covered | `uncle_size_matches_basic_block_size_basis` | I11 |
| 50 | Partial | Proposal prefix test exists; add uncle-prefix exact-fit and partial-fit cases. | I11 |
| 51 | Covered | RaceLost expiry/re-park tests; mechanism is obsolete in target but bounded wake semantics remain. | I1, I9 |
| 52 | Partial | Cache update path is present; add restore-without-reverification counter assertion. | I9, I12 |
| 53 | Covered | `reorg_status_transition_failure_has_no_false_reject_and_replay_converges` injects one failed Gap transition, proves no false reject/status callback, and proves the same authoritative state converges on replay. | I7, I8 |
| 54 | Covered | `get_treats_missing_shard_as_cache_miss` | I4 |
| 55 | Covered | CandidateUncles lowest-height capacity regression. | I11 |
| 56 | Covered | Stale/main-chain/embedded uncle cleanup unit and chain integration tests. | I11 |
| 57 | Accepted | Permanent DAO failure logging is debug-level; retain rate-limited observability. | I11 |
| 58 | Accepted | Selector inconsistency logging is rate-limited debug; invariant auditor must expose counts. | I10, I11 |
| 59 | Partial | Source comments corrected; `pipeline.md` and benchmark documentation are updated with each migration phase. | I12 |
| 60 | Covered | `current_thread_runtime_executes_inline_without_panicking` | I4 |
| 61 | Partial | Defensive assertions exist; target state types must make missing resolved payload unrepresentable. | I1 |
| 62 | Covered | Persisted child-first replay, save/restart, and write-order tests. | I6, I7 |
| 63 | Covered | Remote orphan boundary and recovery test. | I6 |
| 64 | Covered | Same successful-RBF descendant regression as #20. | I2, I3 |
| 65 | Covered | ActiveSet visibility and in-flight-parent regressions. | I1, I6 |
| 66 | Covered | `conflict_recovery_index_stays_consistent` | I1 |
| 67 | Partial | Fee-order behavior is tested; add large fee-order queue scaling benchmark. | I5, I12 |
| 68 | Covered | Duplicate tombstone panic regression. | I1, I4 |
| 69 | Partial | Per-queue limits are tested; target requires aggregate and per-peer residency tests. | I5 |
| 70 | Covered | Escape-hatch rollback and dependency-failure recovery tests. | I3, I10 |
| 71 | Covered | `recover_txs_retries_until_ordered_queue_has_room` | I4 |
| 72 | Covered | `failed_tip_revalidation_recovers_whole_removed_cascade` | I3, I7 |
| 73 | Partial | Selector cache budget tests exist; add adversarial CPFP graph RSS/allocation benchmark. | I5, I12 |
| 74 | Covered | Hold/restore/finalize tests; target permits preliminary fee ordering but grants reversible conflict ownership only after successful verification, then repeats the attack regression. | I1, I5, I9 |
| 75 | Covered | `failed_commit_restores_all_size_evictions_with_original_status_in_lock` proves a rejected commit restores RBF victims plus unrelated prior size evictions, with exact status, before releasing the pool write guard. | I2, I3, I10 |
| 76 | Covered | Reorg attached/detached identity is compared by raw tx hash, not witness hash; `attached_raw_hash_suppresses_detached_witness_variant`. | I4, I7 |
| 77 | Covered | Block-assembler updates use a level-triggered dirty journal before the bounded wake edge; `block_assembler_dirty_journal_is_level_triggered_and_coalesced`. | I11 |
| 78 | Covered | A valid lower RBF candidate arriving behind an unverified winner is held by that winner rather than stranded as `InputsBlocked`; `register_time_loser_is_restored_when_unverified_winner_aborts`. | I1, I9 |
| 79 | Covered | Reorg callbacks are RAII-deferred until `recovery_lock` is released; the injected callback in `reorg_status_transition_failure_has_no_false_reject_and_replay_converges` successfully `try_lock`s recovery state. | I7, I8 |
| 80 | Covered | Queue admission includes active jobs, preventing duplicate CPU work and ActiveSet overwrite. Pop leases use monotonic tokens, and the superseded-at-submit regression proves an old same-epoch finish cannot erase a restored lease. | I1, I4, I5 |
| 81 | Covered | `PipelineEpoch` plus the final in-lock commit check makes clear a linearizable cancellation barrier. `clear_pipeline_cancels_active_commit_without_active_aba`, `stale_deferred_recovery_cannot_resurrect_after_clear`, and epoch-exhaustion coverage lock the boundary. | I1, I2, I4, I7 |
| 82 | Covered | `Completed` verification ownership travels inside held `ResolvedTx`; the superseded restore regression proves recovery is independent of the lossy cache-update channel. | I1, I9, I12 |
| 83 | Covered | Escape-hatch ancestry is recomputed from the surviving graph after a cascade instead of decrementing one; `escape_hatch_stops_after_one_cascade_makes_ancestry_fit`. | I10 |
| 84 | Model-covered | Dependency stale-ticket, exact final-parent wake, generation exhaustion, churn and definitive-failure properties now run against the single coordinator. The old payload-capacity prefix queue is absent by design, and the split dependency prototype is deleted. Production differential coverage remains required. | I1, I5, I6 |
| 85 | Model-covered | Conflict ordering, exact-score tie, independent input domains, waiter revision exhaustion and multi-input handoff now run against the single coordinator; the split conflict prototype and its duplicate lifecycle state are deleted. Production pool-transaction integration remains required. | I1, I9 |
| 86 | Covered | Dispatcher closes and drains the receiver before waiting for permits, so callback/controller re-entry cannot keep shutdown alive indefinitely; channel-close persistence remains the end-to-end anchor. | I4, I8 |
| 87 | Covered | Queue byte limits accept an exact fit (`>` rather than `>=`); queue boundary tests retain the configured budget semantics. | I5 |
| 88 | Covered | Ordered-resolver retry is an atomic active-lease-to-queued handoff; orphan delayed/removal regressions prove active duplicate protection cannot consume the worker's own retry. | I1, I4, I6 |
| 89 | Partial | The isolated `PipelineCoordinator` owns one entry/incarnation/revision and audits entry↔index↔physical-ticket equality. Deadline, accepted-input, source-promotion and bounded fail-closed dependency-cascade transitions plus a 4,000-step state-machine audit are covered. One multi-entry injected unwind is covered; exhaustive boundary injection and production differential coverage remain. | I1, I4 |
| 90 | Model-covered | `administrative_terminal_api_cannot_express_commit_and_releases_all_indexes` and the dedicated typed commit handoffs make commit unavailable to administrative terminalization. Preserve this boundary during production cutover. | I1, I2 |
| 91 | Model-covered | `revision_exhaustion_does_not_consume_the_only_live_queue_ticket` proves checkout preflights revision capacity before consuming the live ticket. Add allocation and production fault injection at cutover. | I4, I6 |
| 92 | Partial | The isolated coordinator has no payload-capacity wait; ID-only peer-bucket stage queues and incarnation deadline tickets use O(1) live sets plus bounded-ratio compaction. Add explicit operation counters and adversarial scaling evidence before cutover. | I5, I12 |
| 93 | Model-covered | `PipelineCoordinator::audit` reconstructs logical live tickets and compares them with both queue live sets and physical ticket membership; focused tests audit every exercised transition family. State-machine coverage is still required by #89. | I1, I4, I6 |
| 94 | Open | Recompute the complete RBF closure and both replacement fee gates under the final pool write guard; registration eligibility is not a durable proof. | I2, I9 |
| 95 | Open | Make the existing pool write guard the only normal-commit membership sequencer, complete coordinator finalization before releasing it, and keep any reorg/persistence chain-operation guard off the normal hot path. | I2, I7, I12 |
| 96 | Partial | Payload plus conservative entry/dependency/ticket/deadline/conflict/accepted-input metadata and global/per-peer active work are continuously charged in the isolated coordinator. Exact production cost calibration and terminal coordinator→outbox charge transfer remain open. | I5 |
| 97 | Model-covered | `unverified_high_fee_work_cannot_own_or_preempt_a_conflict_domain` proves ownership starts only after verification; under-fee and verified-preemption cases are also covered. Add slow/invalid multi-peer production stress and active-work limits before cutover. | I4, I5, I9 |
| 98 | Partial | The isolated coordinator payload types contain no snapshot and invalidation drops the resolved/verified phase, but many-tip release stress and production final-tip/input revalidation are still missing. | I5, I7, I12 |
| 99 | Partial | Per-parent, per-input, per-candidate and global conflict-edge caps are transactional; conflict rechecks and transitive dependency failures use ID-only bounded maintenance queues. Direct children are invalidated before yielding and descendants fail closed while cleanup drains in explicit slices. Operation-count and adversarial scaling evidence remain open. | I4, I5, I6, I12 |
| 100 | Open | Cross-authority queries hold the pool read guard while taking a short coordinator snapshot and never observe the handoff gap; add deterministic query-vs-commit/reorg/clear races. | I1, I2, I7 |
| 101 | Partial | The isolated `EffectOutbox` reserves before mutation, binds FIFO mutation order, retains the active head across retry and continuously charges reserved/queued/active batches. Production publisher supervision, shutdown drain, pool-lock enqueue and common-path scheduling evidence remain open. | I4, I8, I12 |
| 102 | Partial | Count and byte bounds plus stalled-publisher coverage exist in the isolated outbox. Minimal production effect records and coordinator→outbox payload-charge transfer remain open. | I5, I8, I12 |
| 103 | Model-covered | Multi-input all-or-none ownership, committing freeze, capped late waiters, abort recheck and success consumption of the current direct cohort are covered in the isolated coordinator. Production pool-transaction integration remains required. | I1, I2, I9 |
| 104 | Partial | A bounded entry undo guard now wraps dependency wakes/invalidation, accepted-input wakes, verification conflict apply and commit handoff; it restores authoritative entries and deterministically rebuilds derived indexes after error or unwind. `injected_multi_entry_unwind_restores_entries_and_rebuilds_indexes` proves one mid-cascade panic exposes entirely old state. Inject every remaining apply boundary and preserve exact scheduling cursors before production cutover. | I1, I4 |
| 105 | Model-covered | Trusted source promotion releases remote residency/active attribution and retickets queued work in one revision; proposal and normal lanes remain FIFO, and the state-machine audit covers promotion during queued and active ownership. | I1, I5, I9 |
| 106 | Model-covered | Stage queues rotate ID-only peer buckets under global/per-peer active-work caps, with proposal priority in a separate FIFO lane. `peer_rotation_and_active_caps_prevent_a_remote_fifo_prefix_monopoly` proves a long same-peer prefix cannot occupy every worker. Production differential and throughput evidence remain required. | I5, I9, I12 |
| 107 | Model-covered | `definitive_parent_failure_is_fail_closed_and_drained_in_bounded_slices` proves a failed parent synchronously invalidates direct children, blocks an already-committing descendant through dependency ancestry, releases active work, and cleans the transitive tree in bounded slices without resurrection. Production reorg/rejection differential coverage remains required. | I1, I4, I6 |

## Validated security candidates

| Candidate | Disposition | Current evidence | Target requirement |
|---|---|---|---|
| C-racelost-budget | Reportable, fixed at checkpoint | Resolved lifecycle permit, active-job budget test, independent displacement cap. The reviewed target keeps every pre-pool payload and all metadata charges in one coordinator entry, then hands accepted ownership to TxPool. | Integrate the single coordinator, then run independent-input, large-loser/small-winner, slow-winner, and multi-peer memory stress before deleting RaceLost. |
| C-freeloader-rbf | Suppressed | Under-fee candidate cannot pass the current size-fee registration prerequisite; rejected-RBF recovery passes. Follow-up audit nevertheless found that the old barrier ran after releasing the pool lock; exact in-lock rollback removes that transient state independently of the candidate's suppression rationale, and saturated deferred publication can no longer delay the following RBF ownership settlement. | Treat registration as provisional only. Recompute the full conflict closure and fee gates in the mutation-gated pool write transaction, preserving exact rollback plus settle-before-publish ordering. |
| Reorg Gap/uncle proposal exclusion | Confirmed, fixed | Gap transition unit tests plus normal `get_block_template` dependent-tree integration. | Reorg delta sequencing and template revision model must preserve the same integration test without manual proposal blocks. |
| Reorg delta dropped after repeated panic | Confirmed, fixed | Bounded delivery applies backpressure; `retained_message_retries_until_success` and `retained_receiver_preserves_fifo_across_panics` prove the head survives repeated panics and later deltas cannot overtake it. Callback panics are contained separately. | Preserve retained/FIFO delivery while replacing convergence retries with explicit prepare/commit/publish progress in the commit sequencer. |

## Migration and performance gates

1. No legacy mechanism is deleted while any **Open** item affecting it lacks a
   target regression.
2. Every new lifecycle transition is exercised by a pure model/state-machine
   test and at least one production-path differential test.
3. Focused quick A/B runs for each production slice. Medium/full are reserved
   for explicit final acceptance; an interrupted or incomplete run has no
   verdict and is never merged with quick evidence.
4. Throughput geometric mean must not decrease. A negative per-scenario median
   is a rerun condition; a repeated or statistically significant negative result
   blocks the phase.
5. Workload, sample count, safety checks, and timeouts cannot be weakened to
   pass the gate.
