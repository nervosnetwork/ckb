# Tx-Pool Security and Regression Ledger

This ledger is the security gate for the production tx-pool pipeline. It is
derived from the unchanged historical review notes, validated reports, and
reorg/template regression. The coordinator cutover is complete; entries below
are interpreted against the current architecture, not deleted queue/WaitingRoom
implementations.

Status meanings:

- **Covered**: a focused automated regression exists.
- **Model-covered**: the invariant has focused coordinator transition and audit
  coverage; a production-path or integration stress listed in the row remains.
- **Partial**: behavior is covered indirectly or its attack/performance stress
  remains incomplete.
- **Open**: no sufficient automated regression exists yet.
- **Accepted**: intentional compatibility behavior, with its resource/safety
  boundary stated explicitly.
- **Superseded**: the vulnerable mechanism was deleted; the security property
  is enforced and tested at its coordinator/pool/outbox replacement boundary.

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

## Current production cutover evidence

This table is authoritative for the present code. The larger historical table
below retains finding IDs and the evidence available when each item was first
recorded; names belonging to deleted legacy tests are historical labels, not a
claim that those functions still compile.

| Boundary | Current evidence | State |
|---|---|---|
| Single pre-pool owner and typed state | `one_entry_and_revision_own_every_payload_phase_until_candidate_handoff`, `deterministic_state_machine_audits_every_ownership_boundary`, transition fault matrix | Covered |
| Active lease/ABA safety | `active_verification_terminalization_is_causal_and_aba_safe`, revision/sequence exhaustion regressions | Covered |
| Dependency liveness and failure | atomic parent handoff, missing-parent effect, transitive-cycle, invalidation and bounded maintenance regressions | Covered |
| Verified conflict preference | unverified high-fee exclusion, under-fee exclusion, multi-input, exact-tie, source priority and preemption rollback regressions | Covered |
| Final RBF authority and rollback | full closure/fee checks under `TxPool` write guard; size/escape/revalidation rollback and block-assembler delta regressions | Covered |
| Accepted/pre-pool handoff | `pool_removal_demotion_and_winner_handoff_are_one_transaction`; local/reorg synchronous handoff rollback | Covered |
| Historical conflict recovery | generation-tagged cache queue; successful-RBF and administrative-removal cache→coordinator single-owner regressions; adversarial ticket churn | Covered |
| Capacity and attack cost | global/per-peer count+bytes, active work, graph/edge/victim limits, ancestor protection, peer-impossibility and outbox-residency regressions | Covered, algorithmic optimization open |
| Stable-state effects | bounded FIFO outbox, retained active head, reservation-before-mutation, callback lock/re-entry, assembler dirty journal | Covered |
| Shutdown/persistence | state-worker and publisher panic make persistence ineligible; timeout abort/drain ordering and channel-close tests | Covered |
| Reorg/template liveness | Gap reconciliation tests plus `ReorgRecoversDependentPendingTree` normal mining integration; raw-hash identity and retained FIFO delta tests | Covered |
| Local submission semantics | synchronous direct resolve→verify→submit and definitive RPC result | Accepted by design |

## Open final-audit risks

| ID | Risk | Required resolution |
|---|---|---|
| O1 | Coordinator capacity/priority victim discovery still performs repeated whole-live-set scans. Victim count is bounded, but adversarial CPU and `TxPool` lock hold time are not yet production-accepted. | Add equivalent derived priority indexes and operation-count stress before the performance gate. |
| O2 | Cross-component methods rely on a reserved outbox commit being infallible. A panic after a coordinator transition has returned from its internal undo guard is fail-stop rather than a recoverable state/effect transaction. | In the final architecture audit, either make reserved journal commit return an impossible-by-type result or wrap state+effect preparation in one explicit transaction object with fault injection. |
| O3 | A permanently panicking operator callback retains the FIFO outbox head, intentionally preferring no loss over progress. | Document/operator-observe the fail-stop policy or add bounded quarantine without reordering truthful effects. |
| O4 | Conflict recovery drains bounded slices but each handoff currently holds `TxPool` while coordinator admission may run global reconciliation. | Resolve with O1 indexes; add 10k-cache churn/lock-latency stress. |
| O5 | Final performance verdict is intentionally pending. | Run clean repeated A/B only after explicit instruction; do not infer success from unit timing. |
| O6 | Trusted source promotion removes remote budget ownership but currently retains the original remote expiry and loses explicit origin-peer attribution. A promoted Proposal can therefore be terminalized by an obsolete remote deadline, and its relayer cleanup semantics depend on the promoted source. | Split scheduling priority/expiry ownership from immutable ingress attribution. Trusted promotion must atomically cancel remote expiry while retaining the origin peer needed for exactly-once relay settlement; add queued, active, waiting-parent, verified and expiry-race regressions. |

## Historical findings

| # | Status | Regression anchor / required follow-up | Target invariant |
|---:|---|---|---|
| 1 | Covered | `escape_hatch_eviction_drops_cascaded_parents_from_parent_set` | I3, I10 |
| 2 | Covered | `reorg_retain_duplicate_does_not_cascade_dependents` | I4, I7 |
| 3 | Covered | `worker_exits_when_command_channel_dropped` | I4 |
| 4 | Superseded | Double parking is unrepresentable: the coordinator entry is the sole pre-pool owner and `WaitingRoom` no longer exists. | I1, I9 |
| 5 | Superseded | Speculative hold/restore was deleted. Final pool commit recomputes the real conflict closure; no replacement means no victim terminalization. | I2, I9 |
| 6 | Covered | `find_winner_returns_strongest_with_fee_rate` | I9 |
| 7 | Partial | Recording predicate is tested; add end-to-end queue-full resubmission coverage. | I4 |
| 8 | Covered | `remote_duplicate_is_not_relayed_as_reject` | I4, I8 |
| 9 | Covered | `malformed_remote_preflight_bans_peer_and_records_reject` | I4, I8 |
| 10 | Partial | RBF tests traverse the path; target model test must make conflict snapshot/registration atomic. | I2, I9 |
| 11 | Covered | RBF integration family plus recent-reject predicate tests. | I4, I9 |
| 12 | Covered | `pre_check_worker_notifies_relayer_when_ordered_resolve_queue_is_full` | I4 |
| 13 | Superseded | Deferred retry ownership was deleted. Conflict recovery remains cache-owned on `Full`; other coordinator admission failures consume the historical candidate explicitly. | I4 |
| 14 | Covered | Trusted local missing-parent input fails terminally in the coordinator/direct path; remote waiting has bounded expiry. | I4, I6 |
| 15 | Partial | The split prototype covers ready-parent + downstream-full wake-up, but the reviewed coordinator removes internal payload queue `Full`: a residency-charged entry changes an ID-only stage ticket atomically. Port a differential test proving stage handoff cannot lose the transaction. | I4, I6 |
| 16 | Partial | Worker panic guards and callback-panic containment are tested; add injected panic from resolve/verify/commit through final relay and ownership cleanup. | I1, I4 |
| 17 | Covered | `remove_tx_reports_in_progress_for_worker_active_job` | I1 |
| 18 | Covered | `banned_peer_job_is_dropped_by_pre_check_worker` | I4, I5 |
| 19 | Superseded | There are no two executable parking rooms; removal addresses accepted pool, coordinator, and non-executable conflict history by full hash. | I1 |
| 20 | Covered | Successful and failed replacement cascade tests. | I2, I3 |
| 21 | Partial | Reconcile return contract is tested; add reconcile-to-conflict-scheduler cleanup end to end. | I1, I7 |
| 22 | Superseded | Attached commit consumes coordinator ownership through the typed external-commit handoff; no speculative held-candidate owner remains. | I7, I9 |
| 23 | Covered | Attached raw-hash identity suppresses detached replay and external commit removes matching coordinator ownership without a false Dead rejection. | I4, I7 |
| 24 | Covered | `clear_resets_expiry_watermark` | I1 |
| 25 | Covered | `wake_by_winner_keeps_other_reasons_and_stats_intact` | I1 |
| 26 | Accepted | Conflict audit entries intentionally persist; count/byte budgets are mandatory and target separates audit from executable state. | I1, I5 |
| 27 | Covered | `parent_added_after_child_gets_descendant_weight` | I10 |
| 28 | Covered | `conflict_closure_ignores_ghost_link_nodes` | I10 |
| 29 | Covered | `remove_expired_cascades_to_descendants` | I10 |
| 30 | Covered | On-chain reconcile includes cell deps. | I10 |
| 31 | Covered | `counter_drift_is_recovered_by_recompute` and saturating-counter tests. | I5, I10 |
| 32 | Covered | `failed_commit_restores_all_size_evictions_with_original_status_in_lock` asserts the one terminal `Full` event for the rejected candidate and no terminal event for entries restored from the eviction journal. | I4, I5 |
| 33 | Covered | `WaitingParents` remains continuously coordinator-resident and charged, so it is visible as in flight. | I6 |
| 34 | Covered | `dropped_verify_mgr_cancels_its_worker_generation` | I1, I4 |
| 35 | Covered | `save_pool_waits_for_recovery_lock` | I7 |
| 36 | Partial | Same lock path is exercised; add explicit clear-during-recovery final-state test. | I7 |
| 37 | Covered | `full_and_uncle_updates_share_template_serialization_lock` | I11 |
| 38 | Partial | Submit acceptance callback lock test exists; add callback re-entry for reorg pending/proposed/reject batches. | I8 |
| 39 | Partial | Functional restore tests exist; keep a large-restore latency/contention benchmark. | I5, I12 |
| 40 | Covered | `cancel_during_backoff_exits_immediately` | I4 |
| 41 | Superseded | Executable recovery no longer uses a channel. Cache ownership survives cancellation until atomic coordinator admission; shutdown no longer has a channel-drain loss window. | I4 |
| 42 | Superseded | Exact rollback and recovery scheduling both occur under the pool lock. The remaining async channel is best-effort verification cache only and cannot delay ownership settlement. | I4, I5, I8, I12 |
| 43 | Covered | `zero_max_workers_is_clamped_to_one` | I4 |
| 44 | Covered | `dispatcher_channel_close_quiesces_workers_and_persists_pool` proves sender-drop shutdown cancels and joins workers, drains handlers, and persists accepted state before the dispatcher handle completes. | I4, I7 |
| 45 | Covered | `clear_pool_resets_template_and_notifies_miner_immediately` exercises clear → reliable Reset delivery → blank template → immediate miner notification. | I8, I11 |
| 46 | Partial | O(1) index is functional; add compact-block lookup scaling benchmark. | I12 |
| 47 | Covered | `budget_eviction_is_oldest_first` | I5 |
| 48 | Partial | Orphan recovery tests traverse batched lookup; add query-count or scaling benchmark. | I6, I12 |
| 49 | Covered | `uncle_size_matches_basic_block_size_basis` | I11 |
| 50 | Partial | Proposal prefix test exists; add uncle-prefix exact-fit and partial-fit cases. | I11 |
| 51 | Superseded | `RaceLost` and re-park expiry were deleted. One coordinator entry carries conflict waiting, expiry, source, payload, and charge. | I1, I9 |
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
| 65 | Superseded | Per-queue `ActiveSet` was deleted; versioned coordinator active states remain visible to duplicate, parent, RPC, and removal logic. | I1, I6 |
| 66 | Covered | `conflict_recovery_index_stays_consistent` | I1 |
| 67 | Partial | Fee-order behavior is tested; add large fee-order queue scaling benchmark. | I5, I12 |
| 68 | Covered | Duplicate tombstone panic regression. | I1, I4 |
| 69 | Partial | Per-queue limits are tested; target requires aggregate and per-peer residency tests. | I5 |
| 70 | Covered | Escape-hatch rollback and dependency-failure recovery tests. | I3, I10 |
| 71 | Superseded | The ordered recovery queue was deleted. Coordinator `Full` leaves the candidate cache-owned and generation-scheduled for the maintenance tick. | I4 |
| 72 | Covered | `failed_tip_revalidation_recovers_whole_removed_cascade` | I3, I7 |
| 73 | Partial | Selector cache budget tests exist; add adversarial CPFP graph RSS/allocation benchmark. | I5, I12 |
| 74 | Covered | Conflict ownership begins only after successful verification; unverified/under-fee attack regressions and exact pool-lock RBF checks replace hold/restore/finalize. | I1, I5, I9 |
| 75 | Covered | `failed_commit_restores_all_size_evictions_with_original_status_in_lock` proves a rejected commit restores RBF victims plus unrelated prior size evictions, with exact status, before releasing the pool write guard. | I2, I3, I10 |
| 76 | Covered | Reorg attached/detached identity is compared by raw tx hash, not witness hash; `attached_raw_hash_suppresses_detached_witness_variant`. | I4, I7 |
| 77 | Covered | Block-assembler updates use a level-triggered dirty journal before the bounded wake edge; `block_assembler_dirty_journal_is_level_triggered_and_coalesced`. | I11 |
| 78 | Superseded | Unverified work cannot own a conflict domain. Verified candidates remain single coordinator entries; historical accepted-input conflicts remain cache-owned until admission. | I1, I9 |
| 79 | Covered | Reorg effects are journaled under each state mutation and published outside its pool/coordinator guards; mutating synchronous callback re-entry fails fast, while read-only queries may observe the current stable replay slice. `recovery_lock` excludes persistence until replay completes. | I7, I8 |
| 80 | Superseded | Queue admission and active work are one versioned coordinator entry; monotonic leases make late completion ABA-safe without `ActiveSet`. | I1, I4, I5 |
| 81 | Covered | `PipelineEpoch` plus the final in-lock commit check makes clear a linearizable cancellation barrier. Conflict recovery uses the current epoch at cache→coordinator handoff and has no stale deferred job. | I1, I2, I4, I7 |
| 82 | Superseded | Verified payload is retained in the typed coordinator entry; the best-effort verification-cache channel is never an ownership or recovery dependency. | I1, I9, I12 |
| 83 | Covered | Escape-hatch ancestry is recomputed from the surviving graph after a cascade instead of decrementing one; `escape_hatch_stops_after_one_cascade_makes_ancestry_fit`. | I10 |
| 84 | Covered | Dependency stale-ticket, exact final-parent wake, generation exhaustion, churn and definitive-failure properties run against the production coordinator. The old payload-capacity prefix queue and split dependency prototype are deleted. | I1, I5, I6 |
| 85 | Covered | Conflict ordering, exact-score tie, independent input domains, waiter revision exhaustion and multi-input handoff run against the production coordinator and pool-transaction regressions; split conflict lifecycle state is deleted. | I1, I9 |
| 86 | Covered | Dispatcher closes and drains the receiver before waiting for permits, so callback/controller re-entry cannot keep shutdown alive indefinitely; channel-close persistence remains the end-to-end anchor. | I4, I8 |
| 87 | Covered | Queue byte limits accept an exact fit (`>` rather than `>=`); queue boundary tests retain the configured budget semantics. | I5 |
| 88 | Covered | Ordered-resolver retry is an atomic active-lease-to-queued handoff; orphan delayed/removal regressions prove active duplicate protection cannot consume the worker's own retry. | I1, I4, I6 |
| 89 | Partial | The isolated `PipelineCoordinator` owns one closed typed state/incarnation/revision and audits entry↔index↔physical-ticket equality. Deadline, accepted-input, source-promotion and bounded fail-closed dependency-cascade transitions plus a 4,000-step state-machine audit are covered. Seventy-six focused coordinator tests include boundary injection across every current multi-entry transition family; production differential coverage remains. | I1, I4 |
| 90 | Model-covered | `administrative_terminal_api_cannot_express_commit_and_releases_all_indexes` and the dedicated typed commit handoffs make commit unavailable to administrative terminalization. Preserve this boundary during production cutover. | I1, I2 |
| 91 | Model-covered | `revision_exhaustion_does_not_consume_the_only_live_queue_ticket` proves checkout preflights revision capacity before consuming the live ticket. Add allocation and production fault injection at cutover. | I4, I6 |
| 92 | Partial | The isolated coordinator has no payload-capacity wait; ID-only stage tickets and incarnation deadline tickets use O(1) live membership plus bounded-ratio tombstone compaction. Scheduling and global capacity victim discovery deliberately use whole-live-set correctness oracles while semantics stabilize; replace them with equivalent indexed lookup and add operation counters/adversarial scaling evidence before production acceptance. | I5, I12 |
| 93 | Model-covered | `PipelineCoordinator::audit` reconstructs logical live tickets and compares them with both queue live sets and physical ticket membership; focused tests audit every exercised transition family. State-machine coverage is still required by #89. | I1, I4, I6 |
| 94 | Covered | The complete RBF closure and both replacement fee gates are recomputed under the final pool write guard; coordinator conflict ordering remains provisional. | I2, I9 |
| 95 | Covered | The existing pool write guard is the normal-commit membership sequencer and coordinator finalization completes before release; reorg/persistence locking stays off the normal hot path. | I2, I7, I12 |
| 96 | Partial | Payload plus conservative entry/dependency/ticket/deadline/conflict metadata and global/per-peer active work are continuously charged in production. Reserved/queued/active effects remain charged in the outbox; final cost calibration is part of O1/O5. | I5 |
| 97 | Model-covered | `unverified_high_fee_work_cannot_own_or_preempt_a_conflict_domain` proves ownership starts only after verification; under-fee and verified-preemption cases are also covered. Add slow/invalid multi-peer production stress and active-work limits before cutover. | I4, I5, I9 |
| 98 | Partial | The isolated coordinator payload types contain no snapshot and invalidation drops the resolved/verified phase, but many-tip release stress and production final-tip/input revalidation are still missing. | I5, I7, I12 |
| 99 | Partial | Per-parent, per-input, per-candidate and global conflict/pool-input edge caps are transactional and deterministically reconciled; conflict rechecks and transitive dependency failures use ID-only bounded maintenance queues. Direct children are invalidated before yielding, descendants fail closed while cleanup drains in explicit slices, and capacity apply batches have an explicit victim limit. Whole-store victim-selection operation counts and adversarial scaling evidence remain open under #116. | I4, I5, I6, I12 |
| 100 | Partial | Cross-authority queries hold the pool read guard while taking a short coordinator snapshot and production handoff regressions cover removal/commit. Add deterministic query-vs-reorg/clear races in the final audit. | I1, I2, I7 |
| 101 | Covered | Production `EffectOutbox` reserves before mutation, binds FIFO mutation order, retains the active head across retry, continuously charges batches, and is supervised/drained during shutdown. | I4, I8, I12 |
| 102 | Covered | Count/byte bounds, stalled publisher, conservative reservations, critical reorg headroom and shrink-to-actual charge are covered in the production outbox. | I5, I8, I12 |
| 103 | Model-covered | Multi-input all-or-none ownership, committing freeze, capped late waiters, abort recheck and success consumption of the current direct cohort are covered in the isolated coordinator. Production pool-transaction integration remains required. | I1, I2, I9 |
| 104 | Model-covered | A bounded entry undo guard wraps every current multi-entry apply family. Boundary injection covers dependency schedule/drain, parent wake/demotion, accepted-input park/wake, structural/global-capacity admission/eviction/recharge, verified preemption, conflict recheck single/batch, candidate handoff and causal child outcomes; clear reserves output before ownership mutation. Insertion snapshots explicitly encode prior absence. Every unwind restores authoritative entries, budgets, sequence allocators and derived indexes, preserves surviving peer/maintenance order, and exposes no partial result before retry. Repeat the same fault matrix across the TxPool/coordinator/outbox production transaction at cutover. | I1, I4 |
| 105 | Model-covered | Trusted source promotion releases remote residency/active attribution and retickets queued work in one revision; queued proposal re-promotion receives a new FIFO position, while active ownership stays valid. The state-machine audit covers promotion during queued and active ownership. | I1, I5, I9 |
| 106 | Model-covered | Stage queues preserve one configured global order; global/per-peer active-work caps only filter current eligibility. `configured_fifo_order_and_active_caps_prevent_a_remote_prefix_monopoly` proves a capped same-peer prefix cannot occupy every worker without silently replacing FIFO by peer rotation. Production differential and throughput evidence remain required. | I5, I9, I12 |
| 107 | Model-covered | Every definitive parent exit, including expiry and a rejected speculative conflict loser, synchronously invalidates direct children; an accepted commit wakes waiting children in the same transition. Tests also prove an already-committing descendant is blocked through dependency ancestry, later unavailability of another parent cannot resurrect an invalidated child, and transitive cleanup drains in bounded slices. Production reorg/rejection differential coverage remains required. | I1, I4, I6 |
| 108 | Model-covered | Independent `phase + location + candidate` fields allowed audit-detectable but representable illegal combinations, and invalidating a verified candidate retained executable candidate metadata after removing its conflict indexes. `EntryState` now makes legal raw/unverified/candidate-verified/invalidated bundles closed by construction; invalidation drops candidate metadata and recharges to base metadata. Queue/active facts are derived only from private typed state, while the auditor checks dynamic non-empty/subset/cap constraints that enums cannot encode. | I1, I4, I9 |
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
| 119 | Model-covered | Accepted-input capacity compared same-source equal-strength waiters through hash-map iteration, making a higher-source eviction nondeterministic. Eligibility remains strict source/candidate strength, while victim choice now has a separate stable later-sequence/hash tie-break; the equal-waiter regression proves the earlier entry survives. | I4, I5, I9 |

## Validated security candidates

| Candidate | Disposition | Current evidence | Target requirement |
|---|---|---|---|
| C-racelost-budget | Confirmed historical vulnerability; fixed | `RaceLost`, its refunded permit, and split owners are deleted. Every pre-pool payload and all conservative metadata remain charged in one coordinator entry across queued, active, waiting, invalidated, and committing states. Active/global/per-peer and reconciliation-cap tests cover the replacement boundary. | Final multi-peer RSS/CPU stress remains under O1/O5; no legacy mechanism remains to harden. |
| C-freeloader-rbf | Suppressed, high confidence | The alleged under-fee held candidate cannot pass the verified size-fee prerequisite. Independently, final RBF closure and both fee gates are recomputed under the pool write guard, exact rollback happens in-lock, and there is no deferred recovery/publication barrier. | Preserve provisional scheduling vs authoritative commit separation; rerun rejected-replacement integration at final acceptance. |
| Reorg Gap/uncle proposal exclusion | Confirmed historical regression; fixed | Gap demotion/promotion unit tests plus `ReorgRecoversDependentPendingTree`, which mines through normal `get_block_template` and never injects a manual proposal block. Optional detached-block uncles cannot suppress the only recovered proposal path. | Keep the integration in the final serial/integration gate. |
| Reorg delta dropped after repeated panic | Confirmed historical regression; fixed | Bounded delivery retains the FIFO head across panics and prevents later deltas overtaking it. Effects use reserved critical outbox capacity and callback panic containment. | Preserve retained/FIFO delivery and shutdown ineligibility on worker failure. |

## Migration and performance gates

1. No split lifecycle owner may be reintroduced. A new store must declare
   whether it is authoritative, historical, an ID-only index, or best-effort.
2. Every new lifecycle transition requires a coordinator/invariant test and a
   production-path regression at the affected authority boundary.
3. Performance runs are deferred until the correctness architecture and
   production ownership cutovers are nearly complete. At final acceptance run
   focused quick A/B first; medium/full remain explicit-only, and an
   interrupted or incomplete run has no verdict.
4. Throughput geometric mean must not decrease. A negative per-scenario median
   is a rerun condition; a repeated or statistically significant negative result
   blocks the phase.
5. Workload, sample count, safety checks, and timeouts cannot be weakened to
   pass the gate.
6. O1–O6 must be explicitly resolved or accepted with a documented fail-stop
   policy before declaring production readiness.
