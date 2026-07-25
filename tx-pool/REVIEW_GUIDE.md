# Tx-Pool Test-Driven Review Guide

This guide is the reviewer entry point for tx-pool changes. It translates the
architectural invariants in [`security-regression-ledger.md`](security-regression-ledger.md)
into stable `TP-*` behaviors, hostile counterexamples and executable evidence.
The behavior/evidence mapping is generated from
[`review-behaviors.json`](review-behaviors.json); do not edit the generated
region by hand.

## Review workflow

1. Start from every changed production path and select every matching behavior
   row below. A change crossing multiple rows must satisfy all of them.
2. Read the required behavior and hostile case before reviewing the diff. Trace
   the ownership, causal exits, lock/wait order, resource charge and stable
   effects through success, typed rejection, retry, cancellation and panic.
3. Run the row's minimum command and inspect its focused negative assertions.
   Test names are stable review anchors; renaming or deleting one is an explicit
   evidence change, not cleanup.
4. For any behavior change, update the registry, guide prose when needed and a
   focused hostile/failure regression in the same PR. Run all CI gates before
   merge.

The key proof obligation is: every transaction occupies exactly one owning
location at any instant, and resident untrusted state is continuously charged.
TxPool is the accepted-state authority; PipelineCoordinator is the pre-pool
authority; ConflictCache owns only bounded non-executable history; EffectOutbox
owns only bounded stable effects. Derived indexes must never become owners.

## Cross-authority gate

Apply this gate whenever a change touches more than one of coordinator, TxPool,
ConflictCache, EffectOutbox, reorg recovery, persistence or block assembler:

- Identify the linearization point and prove there is no visible ownership gap
  or overlap.
- Write the lock/resource order explicitly. The recovery order is
  `recovery_lock -> effect credit -> TxPool`; publication occurs outside these
  guards.
- Prove every failure is either a typed per-transaction outcome with exact undo,
  a stale no-op, or an invariant/authoritative fail-stop. Hostile input may not
  reach fail-stop.
- Recompute resource equations for payload, metadata, graph edges, active work,
  mutation plans and retained effects before mutation.
- Trace every parent, conflict, chain and administrative exit to its dependent
  wake/invalidation and final external effect.
- Re-check RPC visibility, reorg replay, persistence ordering and normal
  `get_block_template` proposal/commit liveness.

## Command tiers

For a focused change, run the minimum commands in every selected row, then:

```text
python3 devtools/check_tx_pool_review_guide.py
python3 devtools/check_tx_pool_test_layout.py
python3 devtools/check_tx_pool_security_manifest.py
cargo nextest run -p ckb-tx-pool --features internal
cargo clippy -p ckb-tx-pool --all-targets --features internal -- -D warnings
```

Process-level specs are required when their behavior row changes. Benchmark
timing is intentionally a separate final gate: deterministic operation-count
and harness-integrity tests run normally, but checkpoint A/B timing must use
the paired, fingerprinted runner and is performed only when explicitly
authorized. Unit-test duration is never accepted as performance evidence.

## Registered behaviors and evidence

<!-- BEGIN GENERATED: TX_POOL_BEHAVIORS -->

### Behavior index

| ID | Change surfaces | Required behavior | Hostile/failure case | Invariants | Reviewer gate | Performance bound |
|---|---|---|---|---|---|---|
| `TP-OWN-001` Single pre-pool ownership | `tx-pool/src/component/pipeline_coordinator.rs`<br>`tx-pool/src/component/pipeline_coordinator`<br>`tx-pool/src/component/pipeline_runtime.rs` | An admitted transaction has one coordinator entry, one typed location, one admission incarnation and one current lease revision until an atomic handoff transfers sole authority to TxPool. | A stale worker, duplicate admission, failed transition or ABA remove/readmit race must not create two owners, resurrect an old payload or silently erase the current owner. | I1, I4 | - Does every transition consume exactly the state and lease it proves current?<br>- Are every queue, deadline, dependency and conflict structure derived indexes rather than payload owners?<br>- Does failure restore the old owner or publish one explicit terminal outcome? | No second owner map, compensating queue, global post-transition scan or extra hot-path lock. |
| `TP-COMMIT-001` Authoritative commit and handoff | `tx-pool/src/process/submit`<br>`tx-pool/src/component/pipeline_coordinator/commit.rs`<br>`tx-pool/src/pool.rs` | The existing TxPool write guard is the only final membership/RBF sequencer; tentative pool mutation, coordinator handoff, exact rollback and stable effects form one causally ordered boundary. | Concurrent commits, injected handoff failure or panic must not expose a pool/coordinator ownership gap, strand Committing or report success for a rolled-back mutation. | I1, I2, I3, I4, I7, I8, I9 | - Is every final fee/conflict decision recomputed under the pool write guard?<br>- Can any error release the guard before coordinator settlement and pool rollback are exact?<br>- Is an uncertain authoritative mutation escalated instead of downgraded to a transaction reject? | Reuse the existing pool sequencer; do not add a normal-path recovery lock, second commit queue or population-sized reconciliation. |
| `TP-RBF-001` Deterministic RBF preference and rollback | `tx-pool/src/process/submit/rbf_commit.rs`<br>`tx-pool/src/component/pipeline_coordinator/indexes.rs`<br>`tx-pool/src/component/conflict_cache.rs` | Only verified candidates participate in deterministic conflict ordering, while TxPool recomputes the complete replacement closure and both fee gates before atomic victim displacement. | An under-fee, multi-input, dep-group or concurrent candidate must not preempt through speculative state; failed replacement must restore the complete original closure before competitors advance. | I2, I3, I5, I9, I10, I12 | - Is coordinator ordering still provisional rather than an admission verdict?<br>- Are all input and expanded dependency conflicts included in final closure and rollback?<br>- Does every failed path preserve original statuses, accounting and descendant order? | Conflict work stays within indexed bounded cohorts and immutable mutation plans; no full-pool scan under the write guard. |
| `TP-DEP-001` Causal dependency graph | `tx-pool/src/component/pipeline_coordinator/lifecycle.rs`<br>`tx-pool/src/resolved_tx.rs`<br>`tx-pool/src/component/links.rs` | Raw, resolved and accepted dependency edges—including expanded dep-group members—share complete causal semantics: availability wakes children and definitive loss invalidates them atomically. | Late-discovered parents, transitive cycles, stale resolved children or parent replacement must not strand a child, lose its wake edge or let it commit against unavailable inputs. | I4, I5, I6 | - Are input, cell-dep, header-dep and expanded dep-group roles intentionally distinguished?<br>- Does parent success/failure update reverse edges, accounting and child location in one transition?<br>- Are cascade size and maintenance work explicitly bounded? | Use bounded indexed parent/child buckets and maintenance slices; never poll all waiting transactions or scan the pool for dependents. |
| `TP-CACHE-001` Conflict-history ownership and wakeup | `tx-pool/src/component/conflict_cache.rs`<br>`tx-pool/src/service/pipeline_ops.rs`<br>`tx-pool/src/process/reorg.rs` | ConflictCache is one bounded non-executable owner until every indexed input and expanded dependency is available; cache-to-coordinator transfer is atomic and generation-safe. | A release observed before another parent becomes live, duplicate metadata enrichment or high-fanout input must not lose the only future wake, duplicate ownership or cause unbounded pool-lock work. | I4, I5, I6, I7, I9 | - Does the cache retain ownership until all recovery outpoints are live?<br>- Can every chain/pool availability edge re-arm a previously examined entry?<br>- Are discovery, generations and fanout work bounded and fair? | Bound history count/bytes and process indexed recovery in fixed fair slices outside population-sized scans. |
| `TP-BUDGET-001` Continuous hostile-state accounting | `tx-pool/src/component/pipeline_coordinator/capacity.rs`<br>`tx-pool/src/component/effect_outbox.rs`<br>`tx-pool/src/component/conflict_cache.rs` | Global and per-peer count, bytes and active-work budgets continuously charge payload and conservative metadata in every resident state, including bounded terminal effects. | Parking, invalidation, reservation, peer churn or an oversized displacement plan must not refund resident state, evict unrelated stronger work or mutate before proving the bound. | I5, I12 | - Is every owner charged if and only if it is resident?<br>- Are count, bytes, graph edges, victims and active work all bounded before mutation?<br>- Does an impossible peer admission fail before global eviction planning? | Budget checks and victim selection use maintained bounded indexes; no attacker-sized repair on the admission hot path. |
| `TP-WORKER-001` Level-triggered executable readiness | `tx-pool/src/component/pipeline_runtime.rs`<br>`tx-pool/src/service/workers.rs`<br>`tx-pool/src/service/builder.rs` | Readiness is derived after each transition from the authoritative capability-aware checkout predicate; failed ineligible checkout is silent and subscription/respawn re-arms executable work. | A capped peer backlog must not self-wake into mutex-starving livelock; a small-only worker must not consume the only wake for large work; cancellation, zero workers or respawn must not strand executable work. | I4, I5, I12 | - Does notification mean at least one worker of this capability can execute now?<br>- Can a consumed permit be reconstructed from authoritative state after subscribe or respawn?<br>- Does cancellation stop checkout before another self-sustaining wake loop begins? | Readiness checks inspect bounded owner heads/caps only; no polling loop, per-item task or queue-wide scan. |
| `TP-ADMIN-001` Administrative and hostile-peer terminalization | `tx-pool/src/service.rs`<br>`tx-pool/src/service/pipeline_ops.rs`<br>`tx-pool/src/component/recent_reject.rs` | Ban, expiry, clear and malformed-input policy terminalize the current non-committing owner once, release all resource/index state and publish only policy-eligible effects. | An already active malicious peer must not pin an active slot, resurrect through a late lease or turn a typed malformed input into fail-stop or an ineligible relay reject. | I4, I5, I8 | - Does administrative removal make every outstanding lease stale atomically?<br>- Are trusted promotion and immutable ingress attribution preserved deliberately?<br>- Are ban/reject history and relay policy separate explicit decisions? | Administrative removal uses indexed owners and bounded batches; peer fences/history remain bounded and expiring. |
| `TP-EFFECT-001` Reserved stable-state effects | `tx-pool/src/component/effect_outbox.rs`<br>`tx-pool/src/service/effects.rs`<br>`tx-pool/src/callback.rs` | Mutation-coupled effects reserve capacity before state change, enter one FIFO sequence while state is stable and publish outside authority locks; capacity/close waits cannot lose wakeups. | A full or panicking consumer, callback re-entry, close race or check-before-wait window must not expose intermediate state, reorder outcomes, lose charges or sleep forever. | I5, I8, I12 | - Was sufficient effect credit reserved before every coupled mutation?<br>- Is publication outside locks while FIFO ownership and charge remain retained?<br>- Do waiters register before checking and does close use the correct stored/broadcast wake semantic? | One bounded publisher/outbox; no unbounded channel, callback-under-lock, busy retry or task per effect. |
| `TP-REORG-001` Serialized reliable chain transitions | `tx-pool/src/process/reorg.rs`<br>`tx-pool/src/service.rs`<br>`tx-pool/src/service/pipeline_ops.rs` | Reorg deltas remain ordered and retained through retry; recovery_lock serializes complete detached replay, clear and persistence, while final effects describe only authoritative post-replay state. | Repeated callback panic, duplicate detached roots, clear/save races or partial replay must not drop/overtake a delta, expose an ownership gap, deadlock effect credit or persist an intermediate pool. | I1, I2, I4, I7, I8 | - Is lock order recovery_lock then effect credit then TxPool preserved?<br>- Can retry replay an already completed authoritative phase or an obsolete snapshot?<br>- Are duplicate recovery, final-state effects and attached/detached identities exact? | Retain a capacity-one ordered delta and bounded replay slices; no independent recovery worker or full-history duplication. |
| `TP-PERSIST-001` Coherent persistence recovery point | `tx-pool/src/service.rs`<br>`tx-pool/src/component/pool_map.rs`<br>`tx-pool/src/process/reorg.rs` | Persistence snapshots only a coherent accepted pool after complete recovery, orders by the authoritative expanded dependency graph and is disabled only when authoritative mutation may be uncertain. | Save racing detached replay, effect-journal failure or expanded dep-group ordering must not persist half a reorg, lose a recoverable pool or serialize children before required parents. | I7, I8, I10 | - Does save hold recovery serialization across its complete snapshot boundary?<br>- Is PoolMap the only ordering authority, including expanded dependencies?<br>- Does each failure domain make the intended persistence decision? | Shutdown-only clone/sort work stays off admission paths; do not add a continuously maintained persistence projection. |
| `TP-POOL-001` Atomic accepted-pool graph integrity | `tx-pool/src/component/pool_map.rs`<br>`tx-pool/src/component/links.rs`<br>`tx-pool/src/pool.rs` | Pool entries, status counters, dependency links, conflict closure, ancestor/descendant weights and exact rollback journals mutate as one authoritative graph. | Ghost links, counter drift, late-parent insertion, expired-parent cascades or escape-hatch eviction must not corrupt graph weights, preserve impossible children or remove required ancestors. | I10 | - Does one immutable plan cover the complete mutation before any write?<br>- Does the independent audit rebuild agree after success and rollback?<br>- Are required parents distinguished from ordering-only references? | Mutation and audit work is bounded by explicit graph/victim caps; cold repair must never hide hot-path drift. |
| `TP-TEMPLATE-001` Block-template liveness and priority | `tx-pool/src/block_assembler`<br>`tx-pool/src/pool.rs`<br>`tx-pool/src/process/reorg.rs` | Reset and full rebuild remain mutually exclusive with full highest priority; optimistic proposal/transaction updates are revision-safe, and valid recovered transactions always regain a proposal/commit path. | Detached uncle proposals, stale Gap status or an update_full race must not make a valid transaction RPC-pending but forever absent from normal get_block_template mining. | I7, I9, I11 | - Does full/reset retain authority over every optimistic delta and re-dirty skipped generations?<br>- Can uncle proposal filtering exclude the sole proposal path of a recovered transaction?<br>- Are Gap/Pending/Proposed transitions reflected to assembler selection? | Keep optimistic CAS updates and bounded selection; do not serialize every delta behind full rebuild or remove bounded packing safeguards without measurements. |
| `TP-IDENTITY-001` Full transaction and witness identity | `tx-pool/src/process/submit/mod.rs`<br>`tx-pool/src/component/conflict_cache.rs`<br>`tx-pool/src/component/pool_map.rs` | Ownership and duplicate boundaries use full raw hashes, proposal short IDs remain non-authoritative indexes, and verification-cache proofs are keyed only by the exact witness hash through TxVerificationCacheKey. | Short-ID collisions or same-raw/different-witness variants must not alias accepted/cache/history ownership, obtain a false duplicate success or reuse an invalid verification proof during reorg. | I1, I2, I4, I5, I7, I9, I10 | - Is a short ID used only where collision-aware lookup semantics are explicit?<br>- Can any cache call construct a key from raw hash or arbitrary bytes?<br>- Does reorg recovery query the exact transaction witness variant? | Use compact full hashes/typed cache keys without retaining packed backing; collision handling remains indexed and bounded. |
| `TP-PERF-001` Bounded attacker-controlled work | `tx-pool/src/component/pipeline_coordinator/capacity.rs`<br>`tx-pool/src/component/pipeline_coordinator/scheduling.rs`<br>`tx-pool/src/benchmark.rs`<br>`devtools/tx_pool_bench.py` | Owner-head scheduling, victim selection and conflict probing stop at maintained bounds independent of unrelated population; benchmark comparisons use fingerprinted paired checkpoints and reject noisy samples. | A capped peer prefix, stronger suffix or large independent population must not turn one admission/checkout into an O(pool) scan; a noisy or mismatched harness must not claim a performance win. | I5, I12 | - Is operation count bounded by owners/cohort/config rather than resident transaction count?<br>- Did the change add allocation, lock, task, scan or mutable projection to a hot path?<br>- When benchmarking is authorized, are worktree, binary, config, repetitions and spread comparable? | Deterministic operation-count regressions are always required; timing A/B is the separately authorized final gate, never inferred from unit duration. |

### Executable evidence

#### `TP-OWN-001` — Single pre-pool ownership

Minimum command: `cargo nextest run -p ckb-tx-pool --features internal -E 'test(/(one_entry_and_revision|active_verification_terminalization|removed_and_readmitted_hash|queue_sequence_exhaustion|coordinator_invariant_error)/)'`

Rust evidence:

- `active_verification_terminalization_is_causal_and_aba_safe` (I4)
- `coordinator_invariant_error_cannot_be_downgraded_to_transaction_reject` (I4)
- `one_entry_and_revision_own_every_payload_phase_until_candidate_handoff` (I1)
- `queue_sequence_exhaustion_cannot_strand_transition_reservations` (I4)
- `removed_and_readmitted_hash_rejects_the_old_worker_incarnation` (I4)

#### `TP-COMMIT-001` — Authoritative commit and handoff

Minimum command: `cargo nextest run -p ckb-tx-pool --features internal -E 'test(/(pool_removal_invalidation|pipeline_commit_worker|production_pool_coordinator|coordinator_handoff_panic|production_handoff_invariant|second_independent_commit)/)'`

Rust evidence:

- `coordinator_handoff_panic_preserves_pool_recovery_point` (I4, I7)
- `pipeline_commit_worker_waits_for_the_pool_sequencer` (I2)
- `pool_removal_invalidation_and_winner_handoff_are_one_transaction` (I1)
- `production_handoff_invariant_settles_then_fails_closed` (I4, I7)
- `production_pool_coordinator_outbox_fault_matrix_is_atomic` (I1, I3, I4, I8)
- `second_independent_commit_checkout_waits_for_first_terminal_transition` (I9)

#### `TP-RBF-001` — Deterministic RBF preference and rollback

Minimum command: `cargo nextest run -p ckb-tx-pool --features internal -E 'test(/(rbf|replacement|under_fee_candidate|unverified_high_fee|multi_input_verified|derived_conflict|model_random_transitions|failed_tip_revalidation)/)'`

Rust evidence:

- `derived_conflict_relation_has_no_maintenance_gap` (I9)
- `failed_commit_restores_all_size_evictions_with_original_status_in_lock` (I3, I10)
- `failed_tip_revalidation_recovers_whole_removed_cascade` (I3)
- `model_random_transitions_always_match_full_rebuild` (I9)
- `multi_input_verified_candidate_is_all_or_none_and_committing_is_frozen` (I9)
- `pipeline_concurrent_rbf_prefers_highest_fee` (I2)
- `rbf_rejects_dep_group_member_from_replacement_victim` (I9)
- `under_fee_candidate_cannot_become_verified_conflict_state` (I9)
- `unverified_high_fee_work_cannot_own_or_preempt_a_conflict_domain` (I9)

Process-level evidence:

- `rbf-basic`: `test/src/specs/tx_pool/replace.rs::RbfBasic` (I2, I9)
- `rbf-cell-deps`: `test/src/specs/tx_pool/replace.rs::RbfCellDepsCheck` (I2, I9, I10)
- `rbf-concurrency`: `test/src/specs/tx_pool/replace.rs::RbfConcurrency` (I2, I9)
- `rbf-cycling-attack`: `test/src/specs/tx_pool/replace.rs::RbfCyclingAttack` (I5, I9, I12)
- `rbf-proposed-success`: `test/src/specs/tx_pool/replace.rs::RbfReplaceProposedSuccess` (I2, I9)

#### `TP-DEP-001` — Causal dependency graph

Minimum command: `cargo nextest run -p ckb-tx-pool --features internal -E 'test(/(parent_handoff|parent_exit|parent_wait|dependency_cycle|dependency|dep_group|trusted_parent|verified_parent|successful_resolution)/)'`

Rust evidence:

- `accepted_parent_handoff_wakes_waiting_children_atomically` (I6)
- `every_definitive_parent_exit_invalidates_dependents_in_the_same_transition` (I6)
- `local_rbf_commit_demotes_consumer_of_live_expanded_dep_group_member` (I6)
- `raw_parent_wait_extends_a_dep_group_discovered_dependency` (I4, I6)
- `remote_parent_wait_and_unknown_parents_effect_are_one_transition` (I4)
- `successful_resolution_tracks_live_dep_group_members_before_verify` (I6)
- `transitive_dependency_cycle_is_rejected_before_admission` (I6)
- `trusted_parent_invalidation_drops_typed_payload_and_makes_active_verify_lease_stale` (I4, I6)
- `verified_parent_invalidation_is_raw_only_and_releases_conflict_projection` (I5, I6)

#### `TP-CACHE-001` — Conflict-history ownership and wakeup

Minimum command: `cargo nextest run -p ckb-tx-pool --features internal -E 'test(/(conflict_recovery|accepted_entry_metadata|attached_parent_output)/)'`

Rust evidence:

- `accepted_entry_metadata_extends_duplicate_wake_edges` (I5)
- `attached_parent_output_rearms_conflict_cache_after_earlier_release` (I6, I7)
- `conflict_recovery_waits_for_cell_dep_parent_output` (I4, I6)

Process-level evidence:

- `rbf-conflict-recovery`: `test/src/specs/tx_pool/orphan_tx_recovery.rs::RbfOrphanRecovery` (I7, I9)

#### `TP-BUDGET-001` — Continuous hostile-state accounting

Minimum command: `cargo nextest run -p ckb-tx-pool --features internal -E 'test(/(metadata_residency|count_and_byte_limits|escape_hatch_rejects)/)'`

Rust evidence:

- `count_and_byte_limits_cover_reserved_queued_and_active_batches` (I5)
- `escape_hatch_rejects_mutation_larger_than_displacement_bound` (I5, I12)
- `metadata_residency_is_charged_continuously_across_every_payload_phase` (I5)

#### `TP-WORKER-001` — Level-triggered executable readiness

Minimum command: `cargo nextest run -p ckb-tx-pool --features internal -E 'test(/(ineligible_checkout|large_verify_work|worker_resubscribe|cancelled_runtime|zero_verify_worker)/)'`

Rust evidence:

- `cancelled_runtime_does_not_checkout_queued_raw_work` (I4)
- `ineligible_checkout_does_not_self_notify_until_active_owner_settles` (I5, I12)
- `large_verify_work_wakes_only_a_capable_worker_without_losing_readiness` (I5, I12)
- `worker_resubscribe_rearms_authoritative_ready_work` (I4, I12)
- `zero_verify_worker_config_still_runs_remote_pipeline` (I4)

#### `TP-ADMIN-001` — Administrative and hostile-peer terminalization

Minimum command: `cargo nextest run -p ckb-tx-pool --features internal -E 'test(/(banned_peer_revokes|malformed_remote_preflight)/)'`

Rust evidence:

- `banned_peer_revokes_active_remote_lease_and_releases_budget` (I4, I5)
- `malformed_remote_preflight_is_banned_recorded_and_not_relayed` (I4, I8)

#### `TP-EFFECT-001` — Reserved stable-state effects

Minimum command: `cargo nextest run -p ckb-tx-pool --features internal -E 'test(/(effect_credit|fifo_sequence|full_relayer|close_wakes|idle_publisher)/)'`

Rust evidence:

- `close_wakes_every_blocked_capacity_waiter` (I5, I8, I12)
- `fifo_sequence_follows_authoritative_commit_not_reservation_order` (I8)
- `full_relayer_retains_fifo_head_and_outbox_charge` (I8)
- `idle_publisher_observes_close_without_a_later_ready_event` (I8, I12)
- `local_commit_waits_for_effect_credit_before_mutating_pool` (I8)

#### `TP-REORG-001` — Serialized reliable chain transitions

Minimum command: `cargo nextest run -p ckb-tx-pool --features internal -E 'test(/(reorg_status_transition|reorg_retain_duplicate|retained_receiver|clear_during_reorg|cross_authority_query)/)'`

Rust evidence:

- `clear_during_reorg_recovery_owns_the_final_empty_state` (I4, I7, I8)
- `cross_authority_query_is_serialized_with_clear_and_reorg` (I1, I2, I7)
- `reorg_retain_duplicate_does_not_cascade_dependents` (I7)
- `reorg_status_transition_failure_has_no_false_reject_and_replay_converges` (I7)
- `retained_receiver_preserves_fifo_across_panics` (I7)

Process-level evidence:

- `dependent-reorg-chain`: `test/src/specs/tx_pool/reorg_recovers_dependent.rs::ReorgRecoversDependentChain` (I7)
- `dependent-reorg-transactions`: `test/src/specs/tx_pool/reorg_recovers_dependent.rs::ReorgRecoversDependentTxs` (I7)

#### `TP-PERSIST-001` — Coherent persistence recovery point

Minimum command: `cargo nextest run -p ckb-tx-pool --features internal -E 'test(/(save_pool_waits|stable_effect_journal|persisted_file_orders)/)'`

Rust evidence:

- `persisted_file_orders_expanded_dep_group_parents` (I7, I10)
- `save_pool_waits_for_complete_reorg_recovery_point` (I7)
- `stable_effect_journal_failure_preserves_pool_persistence` (I7, I8)

#### `TP-POOL-001` — Atomic accepted-pool graph integrity

Minimum command: `cargo nextest run -p ckb-tx-pool --features internal -E 'test(/(conflict_closure_ignores|status_counter_underflow|escape_hatch_never|test_dep_group|parent_added_after_child|reorg_expiry_cascades)/)'`

Rust evidence:

- `conflict_closure_ignores_ghost_link_nodes` (I10)
- `escape_hatch_never_evicts_a_required_parent` (I10)
- `parent_added_after_child_gets_descendant_weight` (I10)
- `reorg_expiry_cascades_from_expired_parent_to_fresh_child` (I10)
- `status_counter_underflow_recomputes_from_authoritative_entries` (I10)
- `test_dep_group` (I10)

#### `TP-TEMPLATE-001` — Block-template liveness and priority

Minimum command: `cargo nextest run -p ckb-tx-pool --features internal -E 'test(/(reorg_demotes_stale_gap|pending_proposals_filter|full_and_uncle_updates)/)'`

Rust evidence:

- `full_and_uncle_updates_share_template_serialization_lock` (I11)
- `pending_proposals_filter_conflicting_uncle_subtree` (I11)
- `reorg_demotes_stale_gap_to_pending` (I11)

Process-level evidence:

- `normal-mining-reorg-tree`: `test/src/specs/tx_pool/reorg_recovers_dependent.rs::ReorgRecoversDependentPendingTree` (I7, I11)
- `rbf-proposed-template-refresh`: `test/src/specs/tx_pool/replace.rs::RbfRejectReplaceProposed` (I9, I11)

#### `TP-IDENTITY-001` — Full transaction and witness identity

Minimum command: `cargo nextest run -p ckb-tx-pool --features internal -E 'test(/(short_id_collision|complete_hash_identity|verification_cache_isolated|reorg_recovery_reads_cache)/)'`

Rust evidence:

- `complete_hash_identity_retains_short_id_collisions` (I1, I5)
- `conflict_recovery_retries_pool_short_id_collision_without_losing_history` (I1, I4, I9)
- `full_hash_lookup_does_not_alias_a_proposal_short_id_collision` (I1, I10)
- `pool_short_id_collision_is_not_a_successful_duplicate` (I1, I2, I10)
- `reorg_recovery_reads_cache_by_exact_witness_hash` (I1, I7, I9)
- `synchronous_precheck_does_not_alias_short_id_collision_as_duplicate` (I1, I4)
- `verification_cache_isolated_by_witness_hash_not_raw_hash` (I1, I9)

#### `TP-PERF-001` — Bounded attacker-controlled work

Minimum command: `cargo nextest run -p ckb-tx-pool --features internal -E 'test(/(capped_peer_prefix|victim_selection_stops|model_conflict_probe_cost)/)'`

Rust evidence:

- `capped_peer_prefix_selection_is_bounded_by_owners_not_transactions` (I5, I12)
- `conflict_edge_victim_selection_stops_before_stronger_pool_suffix` (I12)
- `global_capacity_victim_selection_stops_before_stronger_pool_suffix` (I12)
- `model_conflict_probe_cost_ignores_independent_population` (I12)

<!-- END GENERATED: TX_POOL_BEHAVIORS -->

## PR evidence checklist

- Changed paths map to all applicable `TP-*` rows; new test seams reference an
  existing behavior ID.
- Focused tests demonstrate both the required outcome and the previous hostile
  counterexample; broad green tests alone are insufficient.
- No lifecycle state, owner, queue, worker, lock, mutable projection, global
  hot-path scan or compensation protocol was added without deleting the
  parallel encoding it replaces and re-running the architecture review.
- Test helpers remain in declared test roots. Production visibility is not
  widened for tests, and every test-only seam is listed in
  [`test-layout-manifest.json`](test-layout-manifest.json).
- The security ledger is updated when a risk is accepted rather than fixed;
  accepted residuals state scope, consequence and future trigger.
- Full correctness gates are green. Performance claims wait for controlled A/B
  evidence and cannot be inferred from a noisy quick run.
