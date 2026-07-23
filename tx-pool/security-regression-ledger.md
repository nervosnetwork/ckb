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
| 89 | Partial | The isolated `PipelineCoordinator` owns one closed typed state/incarnation/revision and audits entry↔index↔physical-ticket equality. Deadline, accepted-input, source-promotion and bounded fail-closed dependency-cascade transitions plus a 4,000-step state-machine audit are covered. Seventy-six focused coordinator tests include boundary injection across every current multi-entry transition family; production differential coverage remains. | I1, I4 |
| 90 | Model-covered | `administrative_terminal_api_cannot_express_commit_and_releases_all_indexes` and the dedicated typed commit handoffs make commit unavailable to administrative terminalization. Preserve this boundary during production cutover. | I1, I2 |
| 91 | Model-covered | `revision_exhaustion_does_not_consume_the_only_live_queue_ticket` proves checkout preflights revision capacity before consuming the live ticket. Add allocation and production fault injection at cutover. | I4, I6 |
| 92 | Partial | The isolated coordinator has no payload-capacity wait; ID-only stage tickets and incarnation deadline tickets use O(1) live membership plus bounded-ratio tombstone compaction. Scheduling and global capacity victim discovery deliberately use whole-live-set correctness oracles while semantics stabilize; replace them with equivalent indexed lookup and add operation counters/adversarial scaling evidence before production acceptance. | I5, I12 |
| 93 | Model-covered | `PipelineCoordinator::audit` reconstructs logical live tickets and compares them with both queue live sets and physical ticket membership; focused tests audit every exercised transition family. State-machine coverage is still required by #89. | I1, I4, I6 |
| 94 | Open | Recompute the complete RBF closure and both replacement fee gates under the final pool write guard; registration eligibility is not a durable proof. | I2, I9 |
| 95 | Open | Make the existing pool write guard the only normal-commit membership sequencer, complete coordinator finalization before releasing it, and keep any reorg/persistence chain-operation guard off the normal hot path. | I2, I7, I12 |
| 96 | Partial | Payload plus conservative entry/dependency/ticket/deadline/conflict/accepted-input metadata and global/per-peer active work are continuously charged in the isolated coordinator. Exact production cost calibration and terminal coordinator→outbox charge transfer remain open. | I5 |
| 97 | Model-covered | `unverified_high_fee_work_cannot_own_or_preempt_a_conflict_domain` proves ownership starts only after verification; under-fee and verified-preemption cases are also covered. Add slow/invalid multi-peer production stress and active-work limits before cutover. | I4, I5, I9 |
| 98 | Partial | The isolated coordinator payload types contain no snapshot and invalidation drops the resolved/verified phase, but many-tip release stress and production final-tip/input revalidation are still missing. | I5, I7, I12 |
| 99 | Partial | Per-parent, per-input, per-candidate and global conflict/pool-input edge caps are transactional and deterministically reconciled; conflict rechecks and transitive dependency failures use ID-only bounded maintenance queues. Direct children are invalidated before yielding, descendants fail closed while cleanup drains in explicit slices, and capacity apply batches have an explicit victim limit. Whole-store victim-selection operation counts and adversarial scaling evidence remain open under #116. | I4, I5, I6, I12 |
| 100 | Open | Cross-authority queries hold the pool read guard while taking a short coordinator snapshot and never observe the handoff gap; add deterministic query-vs-commit/reorg/clear races. | I1, I2, I7 |
| 101 | Partial | The isolated `EffectOutbox` reserves before mutation, binds FIFO mutation order, retains the active head across retry and continuously charges reserved/queued/active batches. Production publisher supervision, shutdown drain, pool-lock enqueue and common-path scheduling evidence remain open. | I4, I8, I12 |
| 102 | Partial | Count and byte bounds plus stalled-publisher coverage exist in the isolated outbox. Minimal production effect records and coordinator→outbox payload-charge transfer remain open. | I5, I8, I12 |
| 103 | Model-covered | Multi-input all-or-none ownership, committing freeze, capped late waiters, abort recheck and success consumption of the current direct cohort are covered in the isolated coordinator. Production pool-transaction integration remains required. | I1, I2, I9 |
| 104 | Model-covered | A bounded entry undo guard wraps every current multi-entry apply family. Boundary injection covers dependency schedule/drain, parent wake/demotion, accepted-input park/wake, structural/global-capacity admission/eviction/recharge, verified preemption, conflict recheck single/batch, owner removal, expiry batch, plain/candidate handoff and their causal child outcomes; clear reserves output before ownership mutation. Insertion snapshots explicitly encode prior absence. Every unwind restores authoritative entries, budgets, sequence allocators and derived indexes, preserves surviving peer/maintenance order, and exposes no partial result before retry. Repeat the same fault matrix across the TxPool/coordinator/outbox production transaction at cutover. | I1, I4 |
| 105 | Model-covered | Trusted source promotion releases remote residency/active attribution and retickets queued work in one revision; queued proposal re-promotion receives a new FIFO position, while active ownership stays valid. The state-machine audit covers promotion during queued and active ownership. | I1, I5, I9 |
| 106 | Model-covered | Stage queues preserve one configured global order; global/per-peer active-work caps only filter current eligibility. `configured_fifo_order_and_active_caps_prevent_a_remote_prefix_monopoly` proves a capped same-peer prefix cannot occupy every worker without silently replacing FIFO by peer rotation. Production differential and throughput evidence remain required. | I5, I9, I12 |
| 107 | Model-covered | Every definitive parent exit, including expiry and a rejected speculative conflict loser, synchronously invalidates direct children; an accepted commit wakes waiting children in the same transition. Tests also prove an already-committing descendant is blocked through dependency ancestry, later unavailability of another parent cannot resurrect an invalidated child, and transitive cleanup drains in bounded slices. Production reorg/rejection differential coverage remains required. | I1, I4, I6 |
| 108 | Model-covered | Independent `phase + location + candidate` fields allowed audit-detectable but representable illegal combinations, and invalidating a verified candidate retained executable candidate metadata after removing its conflict indexes. `EntryState` now makes legal raw/unverified/plain-verified/candidate-verified/invalidated bundles closed by construction; invalidation drops candidate metadata and recharges to base metadata. Queue/active facts are derived only from private typed state, while the auditor checks dynamic non-empty/subset/cap constraints that enums cannot encode. | I1, I4, I9 |
| 109 | Model-covered | Raw ownership previously sat outside the phase state and made replacement accounting ambiguous. Every typed phase bundle now owns the retained raw payload needed for dependency demotion/terminal handoff, and completion charges are explicitly the full resident bundle. Production size calibration and coordinator→outbox transfer remain under #96/#102. | I1, I5 |
| 110 | Model-covered | A due `Committing` entry could remain at the live expiry head and prevent later due entries from expiring; repeated commit/abort also accumulated stale physical deadline tickets without triggering compaction. Commit checkout now suspends its deadline with a separate generation, abort restores the original lifetime and compacts bounded-ratio tombstones, and success consumes it. Head-of-line and 100-cycle churn regressions lock both liveness and residency. | I4, I5, I6 |
| 111 | Model-covered | Dependency-failure and conflict-recheck entries carry one globally monotonic maintenance sequence. Audit proves uniqueness/counter bounds and exact live physical order; rebuild sorts from authoritative state. Larger-hash-first regressions force unwind/rebuild and retain enqueue order, while near-exhaustion injection proves the allocator rolls back atomically. | I1, I4, I6 |
| 112 | Model-covered | Queue tickets carry authoritative monotonic scheduling sequence plus verified size-fee/cycle metadata. Arrival mode is global FIFO, fee mode is descending size-based fee rate, proposal priority is absolute, and active-work limits plus `SmallCycleOnly` are orthogonal eligibility filters. Re-promotion, cap-skipping, proposal-over-fee, fee order, worker filtering, sequence exhaustion and rebuild/audit regressions lock the semantics. Indexed lookup and production differential/throughput evidence remain under #92. | I1, I5, I12 |
| 113 | Model-covered | Parent, verified-conflict and accepted-input buckets use deterministic bounded reconciliation rather than first-fill rejection. Proposal > local > remote; comparable verified candidates then use the already-verified total size-fee score, exact ties retain the earlier entry, and `Committing` is frozen. Multi-input victim unions, explicit `CapacityEvicted` terminal records, causal child invalidation and incoming admission/transition commit atomically. Regressions cover source priority, strictly better replacement, multi-input buckets, frozen commit, explicit terminal output, dependency-ancestor protection and unwind at every new apply boundary. Audit/rebuild also enforce every bucket/global edge limit. Production outbox publication/differential stress remain required. | I5, I6, I9, I12 |
| 114 | Model-covered | Global count/byte capacity no longer remains first-fill in the isolated model. Admission and every resident-bundle growth boundary deterministically evict only weaker/non-committing work, preserve incoming dependency ancestors, return causal terminal records and roll back as one transaction. Per-peer impossibility remains fail-closed and is checked before unrelated global planning. Production outbox binding and indexed victim lookup remain under #102/#116. | I5, I6, I8, I12 |
| 115 | Model-covered | Global conflict and accepted-pool-input edge caps previously remained first-fill across disjoint buckets even after per-bucket reconciliation. Both planners now account for the complete preselected victim union, admit only a strictly stronger verified owner, protect dependency ancestors and atomically return every displaced terminal record. Disjoint-input regressions cover both edge domains. | I5, I6, I9, I12 |
| 116 | Partial | Capacity planning now has explicit transitive-ancestor and maximum-victim bounds, so one transition cannot apply an unbounded cascade. Victim discovery still scans the authoritative entry set and may repeat that scan for a bounded number of victims; replace it with an equivalent derived priority index and prove operation counts under saturated adversarial churn before production acceptance. | I5, I6, I12 |
| 117 | Model-covered | An incoming remote entry whose peer budget cannot fit after already-selected structural victims now fails before global victim planning, preventing unrelated evictions or expensive rollback for a peer-local impossibility. Inner budget checks remain as a defensive invariant. | I5, I6 |
| 118 | Model-covered | Direct self-dependency rejection was insufficient for child-first admission: a later parent could close a transitive cycle in the coordinator graph. Admission now computes the bounded ancestor closure against the incoming owner and rejects the second edge before any index, budget or queue mutation; the cycle regression preserves the first waiting entry and a clean audit. | I1, I4, I6 |
| 119 | Model-covered | Accepted-input capacity compared same-source plain waiters as equal and then selected through hash-map iteration, making a higher-source eviction nondeterministic. Eligibility remains strict source/candidate strength, while victim choice now has a separate stable later-sequence/hash tie-break; the plain-waiter regression proves the earlier equal entry survives. | I4, I5, I9 |

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
3. Performance runs are deferred until the correctness architecture and
   production ownership cutovers are nearly complete. At final acceptance run
   focused quick A/B first; medium/full remain explicit-only, and an
   interrupted or incomplete run has no verdict.
4. Throughput geometric mean must not decrease. A negative per-scenario median
   is a rerun condition; a repeated or statistically significant negative result
   blocks the phase.
5. Workload, sample count, safety checks, and timeouts cannot be weakened to
   pass the gate.
