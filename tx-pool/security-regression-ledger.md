# Tx-Pool Security and Regression Ledger

This ledger is the migration gate for the tx-pool pipeline refactor. It is
derived from the historical review notes in the workspace, the validated
security reports, and the reorg/template regression. The source notes remain
unchanged; this tracked document records what the implementation must preserve.

Status meanings:

- **Covered**: a focused automated regression exists.
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
| I1 | One lifecycle owner | A transaction payload has exactly one owner and one location; indexes contain IDs only. |
| I2 | Authoritative commit | Only the tx-pool commit sequencer can accept/reject RBF or mutate pool membership. |
| I3 | Transactional rollback | Failed replacement restores every removed entry before releasing competing candidates. |
| I4 | No silent loss | Every admitted transaction reaches pool, wait state, explicit rejection, or retryable internal failure. |
| I5 | Bounded untrusted state | Global and per-peer count/byte/active-work limits cover queued, active, and parked states continuously. |
| I6 | Event-driven dependencies | Parent commit/failure wakes or reclassifies children; no child relies solely on polling or expiry. |
| I7 | Reliable chain transitions | Reorg deltas are ordered, tip-checked, retained until success, and never best-effort. |
| I8 | Stable-state effects | Callbacks, relay events, metrics, and notifications run after internal ownership is stable and outside locks. |
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
| 15 | Partial | The target `DependencyScheduler` model covers ready-parent + downstream-full capacity wake-up; production routing and ordered/verify queue integration remain mandatory. | I4, I6 |
| 16 | Partial | Panic guards are tested; add injected panic from resolve/verify/commit through final relay and ownership cleanup. | I1, I4 |
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
| 32 | Open | Add deterministic eviction metric/event assertion; warnings alone are insufficient observability. | I4, I5 |
| 33 | Covered | `parent_parked_in_waiting_room_counts_as_in_flight` | I6 |
| 34 | Covered | `dropped_verify_mgr_cancels_its_worker_generation` | I1, I4 |
| 35 | Covered | `save_pool_waits_for_recovery_lock` | I7 |
| 36 | Partial | Same lock path is exercised; add explicit clear-during-recovery final-state test. | I7 |
| 37 | Covered | `full_and_uncle_updates_share_template_serialization_lock` | I11 |
| 38 | Partial | Submit acceptance callback lock test exists; add callback re-entry for reorg pending/proposed/reject batches. | I8 |
| 39 | Partial | Functional restore tests exist; keep a large-restore latency/contention benchmark. | I5, I12 |
| 40 | Covered | `cancel_during_backoff_exits_immediately` | I4 |
| 41 | Covered | `cancel_drains_deferred_recover_txs` | I4 |
| 42 | Partial | Recovery retry tests exist; add saturated deferred-channel throughput and shutdown stress. | I4, I5, I12 |
| 43 | Covered | `zero_max_workers_is_clamped_to_one` | I4 |
| 44 | Open | Add dispatcher-channel-close persistence regression. | I4, I7 |
| 45 | Open | Add clear/reset-to-miner-notify integration regression. | I8, I11 |
| 46 | Partial | O(1) index is functional; add compact-block lookup scaling benchmark. | I12 |
| 47 | Covered | `budget_eviction_is_oldest_first` | I5 |
| 48 | Partial | Orphan recovery tests traverse batched lookup; add query-count or scaling benchmark. | I6, I12 |
| 49 | Covered | `uncle_size_matches_basic_block_size_basis` | I11 |
| 50 | Partial | Proposal prefix test exists; add uncle-prefix exact-fit and partial-fit cases. | I11 |
| 51 | Covered | RaceLost expiry/re-park tests; mechanism is obsolete in target but bounded wake semantics remain. | I1, I9 |
| 52 | Partial | Cache update path is present; add restore-without-reverification counter assertion. | I9, I12 |
| 53 | Open | Add fault-injected reorg status-transition failure ensuring no false reject event. | I7, I8 |
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
| 74 | Covered | Hold/restore/finalize tests; target replaces this with single-owner conflict scheduling and repeats the attack regression. | I1, I5, I9 |

## Validated security candidates

| Candidate | Disposition | Current evidence | Target requirement |
|---|---|---|---|
| C-racelost-budget | Reportable, fixed at checkpoint | Resolved lifecycle permit, active-job budget test, independent displacement cap. The target models now keep payloads in LifecycleStore and bound conflict edges separately. | Integrate the models, then run independent-input, large-loser/small-winner, slow-winner, and multi-peer memory stress before deleting RaceLost. |
| C-freeloader-rbf | Suppressed | Under-fee candidate cannot pass the current size-fee registration prerequisite; rejected-RBF recovery passes. The target `ReplacementFeeGate` negative test makes under-fee scheduling unconstructible. | Source the typed eligibility proof from the authoritative pool calculation and keep rollback as a commit barrier during integration. |
| Reorg Gap/uncle proposal exclusion | Confirmed, fixed | Gap transition unit tests plus normal `get_block_template` dependent-tree integration. | Reorg delta sequencing and template revision model must preserve the same integration test without manual proposal blocks. |
| Reorg delta dropped after repeated panic | Confirmed open gap | Channel-full delivery now applies backpressure, but the handler still logs and drops an already-received delta after two panics. | Retain the head delta until success or a verified full rebuild; add panic/fault injection and FIFO tip-sequence tests. |

## Migration and performance gates

1. No legacy mechanism is deleted while any **Open** item affecting it lacks a
   target regression.
2. Every new lifecycle transition is exercised by a pure model/state-machine
   test and at least one production-path differential test.
3. Quick benchmarks run for each production change, medium benchmarks for each
   phase, and interleaved multi-run full benchmarks before and after the default
   switch.
4. Throughput geometric mean must not decrease. A negative per-scenario median
   is a rerun condition; a repeated or statistically significant negative result
   blocks the phase.
5. Workload, sample count, safety checks, and timeouts cannot be weakened to
   pass the gate.
