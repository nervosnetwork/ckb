# Tx-Pool Test-Driven Review Guide

This guide is the reviewer entry point for tx-pool changes. It translates the
T1-T16 proof obligations in [`ARCHITECTURE.md`](ARCHITECTURE.md) into stable
`TP-*` behaviors, hostile counterexamples and executable evidence.
The behavior/evidence mapping is generated from
[`review-behaviors.json`](../review-behaviors.json); do not edit the generated
region by hand. File ownership and update commands are defined in
[`VALIDATION.md`](VALIDATION.md). Rows describe required current behavior, not
the history of how it was introduced. Evidence explicitly labelled as an open
counterexample falsifies current production behavior, is mechanically bound to
an `OPEN` finding, and never counts as proof that an invariant is satisfied.

## Review workflow

1. Start from every changed production path and select every matching behavior
   row below. A change crossing multiple rows must satisfy all of them.
2. Read the required behavior and hostile case before reviewing the diff. Trace
   the ownership, causal exits, lock/wait order, resource charge and stable
   effects through success, typed rejection, retry, cancellation, typed system
   fault and foreign-endpoint failure.
3. Run the row's generated focused command and inspect its negative assertions.
   The command is derived from exact discovered test names; renaming or deleting
   an anchor is an explicit evidence change, not documentation cleanup.
4. For any behavior change, update the registry, guide prose when needed and a
   focused hostile/failure regression in the same PR. Run all CI gates before
   merge.
5. Apply the production Rust and mathematical-model gates in this guide to the
   whole changed architecture, not only the edited function: type/error design,
   ownership, async lifetime, API misuse resistance, zero-cost abstraction and
   long-term maintainability are review gates. Because this is
   consensus-critical infrastructure, the same pass must record consensus
   determinism, database/on-disk compatibility, network/RPC compatibility,
   upgrade behavior, performance and allocation impact.

The stable proof obligation is: every retained transaction occupies exactly
one `OwnedTx` location in `TxPoolAuthority.entries`, and every resident resource
is continuously charged. `PreAccepted`, `Accepted` and bounded inert
`ReplacementHistory` are mutually exclusive owner variants. Scheduler,
dependency, membership, resource and effect structures are projections inside
the same authority Apply; checked-out work and read receipts never become owners.

## Authority-transition gate

Apply this gate whenever a change touches `AuthorityStore`, `TxPoolAuthority`,
chain evidence, effect publication, reorg recovery, persistence or block
assembler convergence:

- Identify the linearization point and prove there is no visible ownership gap
  or overlap.
- Prove that `Arc<Snapshot>`, `ChainViewId`, transaction ownership, every
  projection, resource charge, clock and committed effect cross one coherent
  `AuthorityStore` Apply. No authority guard spans work, external I/O or
  `.await`; detached replay is ordinary charged Recovery-source admission.
- Prove every legal outcome is Apply, typed Reject, Backpressure, Stale,
  Duplicate or Cancelled, and every detectable structural fault is typed before Apply.
  Invalid states must first be excluded with private types and ownership.
- Reject production `assert*`, `expect`, `unwrap`, `panic!`, `unreachable!`,
  unchecked indexing/arithmetic and `panic + catch_unwind` control flow.
  Deny `clippy::await_holding_lock` so the no-lock-across-await ownership rule
  is also compiler-enforced.
  Genuinely foreign callbacks/endpoints must be isolated outside authority
  locks and may not select transaction settlement, retry, rollback or recovery.
- Recompute resource equations for payload, metadata, graph edges, active work,
  mutation plans and retained effects before mutation.
- Trace every parent, conflict, chain and administrative exit to its dependent
  wake/invalidation and final external effect.
- Reject diagnostic-only production receipt fields and test-only transition
  mirrors. Inspect sealed Plan inputs before Apply and authoritative/effect
  outputs after Apply; retain post-Apply data only for an actual production
  capability, outside-lock destruction, effect or operational consumer.
- Re-check RPC visibility, reorg replay, persistence ordering and normal
  `get_block_template` proposal/commit liveness.

## Mathematical-model gate

Whenever a change adds or alters a state, transition, batch, queue, wait,
version, resource equation or concurrency boundary, prefer an executable model
over a prose-only rule:

1. Name the trusted environment `E`, legal commands, authoritative state `A`,
   linear-capability state `K`, atomic kernel state `Omega = (A, K)`, full
   protocol state `Sigma`, stable observation cuts and equivalence relation.
   Include every affected RPC, membership, resource, template, persistence,
   relay and exact ordered-effect result.
2. Define total `Step_E(Sigma, event)` for the full protocol and total
   `KernelStep_E(Omega, command)` for its authority/capability sub-transition. Boundary
   protocols may observe a committed effect or coherent read receipt but may
   not retroactively decide the authority result. A batch or fast path is
   incomplete until a pure reference model and differential/property tests establish
   `ObsKernel(CommitBatch_E(Omega, X)) =
   ObsKernel(FoldNoInterleave(KernelStep_E, Omega, Canon_Omega(X)))` from the same
   initial state with no intervening authority Apply. Do not confuse this one
   legal linearization with equality under every concurrent interleaving.
3. Check exact owner/resource conservation: owner and charge-row domains are
   equal, every row equals `charge_record(owner)`, hierarchical aggregates are
   the checked fold of rows, and effect usage is the checked fold of the log.
4. At stable cuts, every Computing owner has exactly one current move-only
   capability and no `(hash, version)` has two. A stale capability may exist,
   but it has no mutation right and must retire in bounded work. Do not use the
   false reverse implication that every live capability names a current owner.
   Also prove free plus held compute permits equals the configured bound,
   retained Computing owners equal charged active work, and Local/TestAccept
   permits create no owner row.
5. For each request, command or capability, state executor/environment fairness
   premises and identify a local finite rank while its evidence epoch is
   stable, or a monotonic level with one named releaser. An unchanged-cut retry,
   timer used as a substitute for a releaser, or repair loop fails review.
6. Record fixed, per-item, per-edge, coupled-component and adversarial bounds.
   Operation-count tests must reject a batched path that silently returns to
   one full authority round trip per transaction; timing benchmarks run only
   after this static proof.
7. At every phase boundary, run a model-delta review. A bad trace accepted by
   model and production is a model gap; one rejected by the model but accepted
   by production is a refinement bug; a violated premise is a boundary bug.
   Reopen dependent proofs after changing the model.
8. Map each operation to one semantic behavior owner and also apply every
   cross-cutting protocol for its discovered domain. Every ordinary controller
   message obeys request-location conservation; every ordered chain command
   obeys the reliable capacity-one protocol. A business-family mapping cannot
   waive either shared law.
9. For a change in implementation slices I1-I6, verify the selected bounded
   semantic exchange entry in `architecture-contract.json`: the component must
   retain one authority, name its exact cost and old owner, and bind every
   invariant to a registered falsifier. A slice cannot become `implemented`
   while its target owner is absent or while an unassigned component remains.

Use Rust types and sealed constructors first, a pure executable state model and
property tests second, and deterministic concurrency/model-checking tools only
for the residual capability/wake protocol. Consensus resolution and script
execution are explicit typed assumptions, not reimplemented in the model.
Markdown equations explain the proof boundary but are never sufficient
evidence by themselves. The architecture contract and generated evidence graph
must reject a new proof obligation that has no model/test owner.

## Develop-comparison gate

Architecture necessity is reviewed against the immutable
`develop@91b97ab5f67fea203fdc5e5d6fbc19a5e0f8b987` cut, not against a prose
recollection or the current topology. Run:

```text
python3 tx-pool/scripts/check_develop_refinement.py
cargo nextest run -p ckb-tx-pool --features internal --lib -E 'test(/develop_/)'
```

The source gate validates the exact baseline tree and derives the named call
orders; the nextest slice executes the corresponding negative observation.
For every case, `architecture-contract.json#develop_refinement` records one of
three decisions: `single_authority_required`, `local_correction_sufficient` or
`intentional_compatibility`. A locally correctable historical bug cannot be
used as evidence that the entire UAK is necessary. Conversely, a common owner
is justified only when the local alternative would recreate a distributed
owner/version/charge/effect protocol across the same handoffs.

Review the current theorem and its cost beside each negative witness. A state,
lock, queue, task, log or version that merely transfers the legacy risk or has
no strictly stronger observation remains a deletion/fusion candidate. This
comparison is static and semantic; benchmark parity cannot repair a failed
refinement claim.

## Command tiers

For a focused change, run the generated commands in every selected row, then:

```text
python3 tx-pool/scripts/check_all.py
cargo nextest run -p ckb-tx-pool --features internal
cargo clippy -p ckb-tx-pool --all-targets --features internal -- -D warnings
```

When tx-pool operational metrics change, also run:

```text
cargo nextest run -p ckb-metrics
cargo clippy -p ckb-metrics --all-targets -- -D warnings
```

Metric labels must remain static and low-cardinality. Their values may only
project already-maintained authority counters after locks are released; metric
availability must never affect a transaction or service outcome.

Process-level specs are required when their behavior row changes and must be
run through the generated `make integration CKB_TEST_ARGS='...'` command, not
by invoking a possibly stale `ckb-test` or `ckb` binary directly. The
`[integration]` inventory, behavior mapping and executable runner list must
agree. Benchmark timing is intentionally a separate final gate: deterministic
operation-count and harness-integrity tests run normally, but controlled A/B
timing must use the paired, fingerprinted runner described in
[`BENCHMARK.md`](BENCHMARK.md). Unit-test duration is never accepted as
performance evidence.

All process nodes expose RPC on loopback addresses. The shared integration
client therefore disables system proxy discovery explicitly; reintroducing
ambient proxy routing would add an unrelated external failure domain and can
turn a repository-wide run into false 30-second RPC timeouts. This is harness
isolation, not permission to suppress or filter a failing spec.

Chain-only Rust fixtures must explicitly retire the dormant tx-pool builder
through `disable_tx_pool_and_take_relay_receiver` before starting chain work.
Keeping an unstarted receiver alive is not a harmless mock: the reliable,
capacity-one best-tip channel will correctly backpressure its second delta.
Production-like fixtures must start the tx-pool instead. The layout validator
rejects direct relay-receiver extraction from `sync/src/tests` so this topology
choice cannot silently drift back into a hang.

## Registered behaviors and evidence

<!-- BEGIN GENERATED: TX_POOL_BEHAVIORS -->

### Managed process suite

The 19 focused security anchors are the minimum process gate for the mapped behavior rows:

`python3 tx-pool/scripts/run_managed_integration.py --anchors`

The complete tx-pool impact universe contains 150 specs. Integration and release CI run the exact inventory through:

`python3 tx-pool/scripts/run_managed_integration.py`

The security validator checks the same `[integration]` inventory against the executable `ckb-test --list-specs` output in integration CI. The universe deliberately includes mining, RPC, relay, fork/reorg, DAO and hardfork transaction-ingress boundaries instead of treating `test/src/specs/tx_pool` as complete.

| Integration source | Managed specs |
|---|---|
| `test/src/specs/tx_pool/collision.rs` | `ConflictInGap`, `ConflictInPending`, `ConflictInProposed`, `DuplicatedTransaction`, `RemoveConflictFromPending`, `SubmitConflict`, `TransactionHashCollisionDifferentWitnessHashes` |
| `test/src/specs/tx_pool/dead_cell_deps.rs` | `CellBeingCellDepAndSpentInSameBlockTestGetBlockTemplate`, `CellBeingCellDepAndSpentInSameBlockTestGetBlockTemplateMultiple`, `CellBeingCellDepThenSpentInSameBlockTestSubmitBlock`, `CellBeingSpentThenCellDepInSameBlockTestSubmitBlock` |
| `test/src/specs/tx_pool/declared_wrong_cycles.rs` | `DeclaredWrongCycles`, `DeclaredWrongCyclesAndRelayAgain`, `DeclaredWrongCyclesChunk` |
| `test/src/specs/tx_pool/depend_tx_in_same_block.rs` | `DepentTxInSameBlock` |
| `test/src/specs/tx_pool/descendant.rs` | `HandlingDescendantsOfCommitted`, `HandlingDescendantsOfProposed`, `ProposeOutOfOrder`, `ProposeTransactionButParentNot`, `SubmitTransactionWhenItsParentInGap`, `SubmitTransactionWhenItsParentInProposed` |
| `test/src/specs/tx_pool/different_txs_with_same_input.rs` | `DifferentTxsWithSameInputWithOutRBF` |
| `test/src/specs/tx_pool/get_raw_tx_pool.rs` | `GetRawTxPool` |
| `test/src/specs/tx_pool/limit.rs` | `SizeLimit`, `TxPoolLimitAncestorCount` |
| `test/src/specs/tx_pool/local_test_submission.rs` | `LocalTestSubmissionIsDirect` |
| `test/src/specs/tx_pool/orphan_tx.rs` | `OrphanTxAccepted`, `OrphanTxRejected`, `TxPoolOrphanDoubleSpend`, `TxPoolOrphanNormal`, `TxPoolOrphanPartialInputUnknown`, `TxPoolOrphanReverse`, `TxPoolOrphanUnordered` |
| `test/src/specs/tx_pool/orphan_tx_recovery.rs` | `RbfOrphanRecovery` |
| `test/src/specs/tx_pool/pool_persisted.rs` | `PoolPersisted` |
| `test/src/specs/tx_pool/pool_reconcile.rs` | `PoolReconcile`, `PoolResolveConflictAfterReorg` |
| `test/src/specs/tx_pool/pool_resurrect.rs` | `InvalidHeaderDep`, `PoolResurrect` |
| `test/src/specs/tx_pool/proposal_expire_rule.rs` | `ProposalExpireRuleForCommittingAndExpiredAtOneTime` |
| `test/src/specs/tx_pool/remove_tx.rs` | `RemoveTx` |
| `test/src/specs/tx_pool/reorg_proposals.rs` | `ReorgHandleProposals` |
| `test/src/specs/tx_pool/reorg_recovers_dependent.rs` | `ReorgRecoversDependentChain`, `ReorgRecoversDependentPendingTree`, `ReorgRecoversDependentTxs` |
| `test/src/specs/tx_pool/replace.rs` | `RbfBasic`, `RbfCellDepsCheck`, `RbfChildPayForParent`, `RbfConcurrency`, `RbfContainInvalidCells`, `RbfContainInvalidInput`, `RbfContainNewTx`, `RbfCyclingAttack`, `RbfEnable`, `RbfOnlyForResolveDead`, `RbfRejectReplaceProposed`, `RbfReplaceProposedSuccess`, `RbfSameInput`, `RbfSameInputwithLessFee`, `RbfTooManyDescendants`, `SendConflictTxToRelay`, `SendConflictTxToRelayRBF` |
| `test/src/specs/tx_pool/same_cell_as_input_and_cell_dep.rs` | `SameCellAsInputAndCellDep` |
| `test/src/specs/tx_pool/send_defected_binary.rs` | `send_defected_binary_do_not_reject_known_bugs`, `send_defected_binary_reject_known_bugs` |
| `test/src/specs/tx_pool/send_large_cycles_tx.rs` | `LoadProgramFailedTx`, `NotifyLargeCyclesTx`, `RelayWithWrongTx`, `SendLargeCyclesTxInBlock`, `SendLargeCyclesTxToRelay` |
| `test/src/specs/tx_pool/send_low_fee_rate_tx.rs` | `SendLowFeeRateTx` |
| `test/src/specs/tx_pool/send_multisig_secp_tx.rs` | `send_multisig_secp_tx_use_dep_group_data_hash`, `send_multisig_secp_tx_use_dep_group_type_hash` |
| `test/src/specs/tx_pool/send_secp_tx.rs` | `CheckTypical2In2OutTx`, `send_secp_tx_use_dep_group_data_hash`, `send_secp_tx_use_dep_group_type_hash` |
| `test/src/specs/tx_pool/send_tx_chain.rs` | `SendTxChain`, `SendTxChainRevOrder` |
| `test/src/specs/tx_pool/txs_relay_order.rs` | `TxsRelayOrder` |
| `test/src/specs/tx_pool/valid_since.rs` | `ValidSince` |
| `test/src/specs/mining/basic.rs` | `BlockTemplates`, `MiningBasic` |
| `test/src/specs/mining/fee.rs` | `FeeOfMaxBlockProposalsLimit`, `FeeOfMultipleMaxBlockProposalsLimit`, `FeeOfTransaction`, `MalformedTx`, `ProposeButNotCommit`, `ProposeDuplicated` |
| `test/src/specs/mining/proposal.rs` | `AvoidDuplicatedProposalsWithUncles` |
| `test/src/specs/mining/uncle.rs` | `PackUnclesIntoEpochStarting`, `UncleInheritFromForkBlock`, `UncleInheritFromForkUncle` |
| `test/src/specs/rpc/get_block_template.rs` | `RpcGetBlockTemplate` |
| `test/src/specs/rpc/get_pool.rs` | `TxPoolEntryStatus` |
| `test/src/specs/rpc/submit_block.rs` | `RpcSubmitBlock` |
| `test/src/specs/rpc/truncate.rs` | `RpcTruncate` |
| `test/src/specs/relay/compact_block.rs` | `BlockTransactionsRelayParentOfOrphanBlock`, `CompactBlockEmpty`, `CompactBlockEmptyParentUnknown`, `CompactBlockLoseGetBlockTransactions`, `CompactBlockMissingFreshTxs`, `CompactBlockMissingNotFreshTxs`, `CompactBlockMissingWithDropTx`, `CompactBlockPrefilled`, `CompactBlockRelayLessThenSharedBestKnown`, `CompactBlockRelayParentOfOrphanBlock` |
| `test/src/specs/relay/get_block_proposal_process.rs` | `ProposalRespondSizelimit` |
| `test/src/specs/relay/too_many_unknown_transactions.rs` | `TooManyUnknownTransactions` |
| `test/src/specs/relay/transaction_relay.rs` | `RelayInvalidTransaction`, `RelayInvalidTransactionResumable`, `TransactionRelayBasic`, `TransactionRelayConflict`, `TransactionRelayEmptyPeers`, `TransactionRelayTimeout` |
| `test/src/specs/relay/transaction_relay_low_fee_rate.rs` | `TransactionRelayLowFeeRate` |
| `test/src/specs/sync/block_sync.rs` | `BlockSyncDuplicatedAndReconnect`, `BlockSyncForks`, `BlockSyncFromOne`, `BlockSyncNonAncestorBestBlocks`, `BlockSyncOrphanBlocks`, `BlockSyncRelayerCollaboration`, `BlockSyncWithUncle`, `HeaderSyncCycle`, `RequestUnverifiedBlocks`, `SyncTooNewBlock` |
| `test/src/specs/sync/chain_forks.rs` | `ChainFork1`, `ChainFork2`, `ChainFork3`, `ChainFork4`, `ChainFork5`, `ChainFork6`, `ChainFork7`, `ForkedTransaction`, `ForksContainSameTransactions`, `ForksContainSameUncle`, `LongForks` |
| `test/src/specs/sync/sync_and_mine.rs` | `InboundMinedDuringSync`, `InboundSync`, `OutboundMinedDuringSync`, `OutboundSync` |
| `test/src/specs/dao/dao_tx.rs` | `WithdrawDAO`, `WithdrawDAOWithOverflowCapacity` |
| `test/src/specs/dao/satoshi_dao_occupied.rs` | `DAOWithSatoshiCellOccupied`, `SpendSatoshiCell` |
| `test/src/specs/hardfork/v2021/cell_deps.rs` | `CheckCellDeps` |
| `test/src/specs/hardfork/v2021/since.rs` | `CheckAbsoluteEpochSince`, `CheckRelativeEpochSince` |
| `test/src/specs/hardfork/v2021/vm_b_extension.rs` | `CheckVmBExtension` |
| `test/src/specs/hardfork/v2021/vm_version1.rs` | `CheckVmVersion1` |
| `test/src/specs/hardfork/v2023/vm_version2.rs` | `CheckVmVersion2` |

### Generated task and capability interruption matrix

The architecture contract owns the selected interruption points. Each row binds one production owner and exact Nextest evidence; cancellation introduces no test-only production state. The complete matrix runs with `cargo nextest run -p ckb-tx-pool --features internal`.

| Interruption point | Task role | Production owner | Required capability disposition | Invariants | Exact Nextest evidence |
|---|---|---|---|---|---|
| `validation_before_plan` | `direct_validator` | `tx-pool/src/authority/validation.rs`: `DirectAdmissionValidation`, `DirectAdmissionValidationOutcome` | Validation reads one coherent authority cut and yields sealed evidence without mutation authority; cancellation before planning leaves ownership, charge and effects unchanged. | T1, T2, T6, T11 | `authority::tests::validation::uak_direct_validation_shares_the_final_validator_without_mutation_authority` |
| `prepared_plan_before_apply` | `authority_caller` | `tx-pool/src/authority/plan/settlement.rs`: `SettlementPlan`, `plan_settlement` | Dropping a fully prepared bounded membership plan is semantically mutation-free; no owner, charge, scheduler fact, clock or effect has moved. | T2, T3, T5, T6 | `authority::tests::foundation::uak_independent_plan_drop_and_batch_clock_failure_are_mutation_free` |
| `applied_before_effect_io` | `effect_publisher` | `tx-pool/src/authority/effect.rs`: `CommittedEffect`, `EffectLog` | Apply commits the terminal owner outcome and immutable effect together; dropping an unprocessed publication receipt retains the exact charged FIFO head. | T2, T3, T6, T7 | `authority::tests::foundation::uak_terminal_outcome_and_effect_commit_together`<br>`authority::tests::effect::uak_effect_receipt_preserves_sequence_and_charge` |
| `worker_compute_interrupted` | `compute_worker` | `tx-pool/src/authority/work.rs`: `CheckedOutWork`, `ComputeSettlement` | Cancellation or a closed assignment transport returns or settles the exact checked-out capability once and restores queued work without leaking active-work charge. | T2, T6, T10, T15 | `authority::tests::foundation::uak_runner_cancellation_settles_one_exact_work_capability_before_exit`<br>`authority::tests::compute_coordinator::uak_closed_assignment_transport_returns_the_exact_checked_out_capability` |
| `publisher_io_interrupted` | `effect_publisher` | `tx-pool/src/authority/publisher.rs`: `run_claimed_authority_effect_publisher`, `publish_committed_effect_batch` | Cancellation during external I/O leaves tentative progress at the authoritative FIFO head so the sole replacement publisher resumes without loss or duplicate settlement. | T2, T7, T10 | `authority::tests::publisher::uak_cancelled_publisher_settles_its_tentative_cursor_to_the_fifo_head` |
| `maintenance_interrupted` | `maintenance_worker` | `tx-pool/src/authority/worker.rs`: `run_maintenance_driver`, `run_maintenance_round` | Maintenance owns no transaction outside each bounded Apply; cancellation exits cleanly, while every preexisting expiry and dependency level drains fairly before quiescence. | T4, T8, T10 | `authority::tests::worker::uak_maintenance_driver_fairly_drains_every_preexisting_level`<br>`authority::tests::worker::uak_idle_maintenance_driver_waits_instead_of_spinning` |
| `generation_shutdown` | `topology_owner` | `tx-pool/src/authority/topology.rs`: `AuthorityTaskTopology`, `shutdown` | Shutdown publishes the absorbing verification stop, joins authority workers, closes and drains committed effects, and only then exposes persistence eligibility. | T2, T7, T9, T10, T12 | `authority::tests::topology::uak_topology_shutdown_stops_the_paired_verification_generation`<br>`authority::tests::topology::uak_topology_clean_shutdown_drains_effects_before_persistence` |

### Behavior index

| ID | Implementation owners | Required behavior | Hostile/failure case | Invariants | Reviewer gate | Performance bound |
|---|---|---|---|---|---|---|
| `TP-OWN-001` Single transaction ownership and ABA safety | `tx-pool/src/authority/state.rs`: `enum OwnedTx`, `enum PreAcceptedPhase`, `EntryVersion`<br>`tx-pool/src/authority/work.rs`: `struct SettlementToken`, `enum CheckedOutWork`<br>`tx-pool/src/authority/plan.rs`: `struct TxPoolAuthority`, `struct PreparedApply` | Each raw transaction hash has zero or one OwnedTx in TxPoolAuthority.entries. Compute consumes one move-only work value bound to the exact owner version and Computing phase; chain proof remains bound to its exact view, while other asynchronous receipts carry their own typed generation or source cut. Queues, workers, receipts and effects never own lifecycle state. | Duplicate ingress, source promotion, stale completion, remove/readmit ABA, clear or reorg must not create a second owner, erase the current owner or leave an uncharged payload. | T1, T2, T3, T4, T5, T6, T7, T8, T10, T11 | - Can any payload exist outside entries after an authority guard opens?<br>- Does every stale completion return without semantic mutation?<br>- Can a new phase or owner be assembled without its exact charge and projections? | One owner map and one short authority transition; no compensating owner scan or second lifecycle lock. |
| `TP-COMMIT-001` Read-only Plan and total Apply | `tx-pool/src/authority/plan.rs`: `struct PreparedApply`, `fn plan_final_admission`, `fn apply_membership`, `struct IndependentDelta`<br>`tx-pool/src/authority/runtime.rs`: `fn try_drive_ready`, `fn complete_ready_batch` | All policy, stale, resource, membership and effect decisions finish before a single-use PreparedApply commits owner, projection, charge, clocks and effects. Apply is total; dropped or stale plans are semantically mutation-free. | Concurrent Ready work, chain-view ABA, RBF rejection, capacity pressure, effect saturation or allocation failure must not expose partial membership or require rollback. | T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14, T16 | - Does Plan change any authoritative semantic fact?<br>- Can Apply return an ordinary failure or allocate fallibly?<br>- Are independent batches proven commuting, unique and bounded before Apply? | Fallible work and large destruction stay outside Apply; independent verified transactions may commit in bounded commuting batches. |
| `TP-RBF-001` Atomic deterministic replacement | `tx-pool/src/authority/plan/membership/rbf.rs`: `validate_no_new_unconfirmed_inputs`, `validate_no_victim_dependencies`, `validate_replacement_fee`<br>`tx-pool/src/authority/plan/membership.rs`: `MembershipReject`, `PreparedMembership` | RBF computes the complete bounded victim and descendant closure, all dependency restrictions and both fee gates against one coherent virtual membership before one total replacement Apply. | An under-fee, over-bound, new-unconfirmed-input, victim-dependent, self-evicting or concurrent candidate must leave every existing owner and aggregate unchanged. | T1, T2, T3, T4, T6, T7, T8, T9, T10, T11, T12, T13 | - Are all victims and descendants included exactly once before fees are evaluated?<br>- Can any victim move before the winner and complete history disposition are known?<br>- Is every positive chain-input premise explicit and exact? | Conflict work follows bounded indexes and cohorts; no speculative removal, undo engine or unbounded full-pool scan. |
| `TP-DEP-001` Exact dependency and level-triggered progress | `tx-pool/src/authority/dependency.rs`: `struct DependencyFrontier`, `struct DependencyMaintenanceTicket`, `struct DependencyMaintenancePlan`, `enum DependencyMaintenanceStep`<br>`tx-pool/src/authority/resolver.rs`: `collect_missing_against_cut`, `resolve_candidate` | Canonical input, cell-dep, header and expanded dep-group evidence drives one DependencyFrontier. Missing observations carry an exact cut; availability and definitive loss advance the same level in the owner-changing Apply. Every stable-epoch maintenance Apply carries a sealed nonempty successor and strictly decreases the finite edge-plus-marker rank. | Parent death, repeated availability, source promotion, high fanout, late dep-group discovery or coalesced wakes must not strand a child, accept stale evidence or spin indefinitely. | T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13 | - Does every producer loss reach every surviving indexed consumer exactly once?<br>- Does a waiter subscribe before checking the authoritative level?<br>- Are fanout and maintenance work sliced by explicit bounds? | Indexed key-scoped maintenance replaces polling and population scans; each maintenance step has a fixed work bound. |
| `TP-CACHE-001` Bounded replacement history and recovery | `tx-pool/src/authority/state.rs`: `struct ReplacementHistoryEntry`, `ReplacementHistoryCharge`<br>`tx-pool/src/authority/plan.rs`: `fn plan_replacement_history_admission`, `ReplacementHistoryLimit`<br>`tx-pool/src/authority/plan/chain_transition.rs`: `fn chain_dependency_events`, `chain_available`<br>`tx-pool/src/authority/plan/membership.rs`: `fn spender_after` | Only an actually Accepted RBF victim can become inert charged ReplacementHistory. It observes exact final blockers, and chain-layer availability becomes a wake only after the same Apply's final Accepted overlay has no spender. History has no scheduler/source/peer/deadline, remains private to live RPC/template/persistence, is the sole source of the legacy conflicted hash projection, and re-enters full validation on recovery. Failed candidates terminalize into per-hash reject evidence without retained ownership. | A policy-rejected loser, remote candidate, same-Apply victim wake, partial blocker release, unrelated chain change or saturated history budget must not create executable ghost work, hidden residency, false wake or partial retained history. | T1, T2, T3, T4, T6, T7, T8, T9, T10, T11, T12, T13 | - Is the constructor reachable only from successful Accepted-victim displacement?<br>- Does wake require a newer level for every retained blocker?<br>- Does saturation drop the complete optional set while preserving the winner? | History has independent count/byte/edge bounds and no active-work or scheduler cost. |
| `TP-BUDGET-001` Continuous hostile-resource accounting | `tx-pool/src/authority/resources.rs`: `struct ResourceVector`, `struct ResourceLedger`, `struct OrderedResourceProjection`, `struct ResourceLimits`, `enum ResourceError`, `struct ComputeLimits`, `struct ComputeGrant`, `fn compute_grant`, `fn retained_charge`, `fn active_work_availability`<br>`tx-pool/src/authority/runtime.rs`: `struct ComputeGate`, `AuthorityComputeExecutionPermit` | Every owner continuously carries exact entry, byte, edge, active-work and compute-reservation charges, with accepted, remote, per-peer and replacement-history sublimits validated by checked construction. The resource ledger is the sole compiler of a sealed compute grant, and worker admission plus settlement use its exact total-retained byte and edge units rather than comparing payload bytes with retained residency. | Payload growth, expanded dependency fanout, promotion, active work, ghost charge, arithmetic overflow or peer churn must fail before mutation and cannot escape or double charge across phases. | T1, T3, T4, T5, T6, T7, T8, T10, T11 | - Does owner existence imply exactly one ledger row and vice versa?<br>- Is attacker-shaped growth reserved before it is retained?<br>- Are all sums and limit hierarchies checked? | Accounting is sparse and transition-local; no global recount on ordinary ingress or settlement. |
| `TP-WORKER-001` Capability-owned workers and progress | `tx-pool/src/authority/plan.rs`: `struct AuthorityWakeTransition`<br>`tx-pool/src/authority/plan/compute_exchange.rs`: `struct ComputeExchangeDelta`, `fn apply_compute_exchange`<br>`tx-pool/src/authority/compute_coordinator.rs`: `struct ComputeCoordinator`, `fn spawn_compute_exchange`<br>`tx-pool/src/authority/exchange.rs`: `struct ComputeWorkerGrant`, `enum ComputeWorkerSlot`<br>`tx-pool/src/authority/scheduler.rs`: `struct FairFrontier`, `struct SchedulerExchangeWave`, `struct SchedulerWaveCursor`<br>`tx-pool/src/authority/runtime.rs`: `struct AuthoritySignals`, `fn publish_post_commit`, `fn publish_wake`, `fn exchange_compute`, `fn execute_compute`<br>`tx-pool/src/authority/worker.rs`: `fn spawn_workers`, `run_ready_driver`, `run_maintenance_driver` | One bounded coordinator owns the stable compute-slot partition and is the only retained-compute caller of the authority exchange; workers only execute assigned jobs outside the store guard and return one move-only completion. Idle + Assigned + Finished equals the configured retained slot count, and every Finished slot owns exactly one queued completion. Fair count grants, stable worker roles and authority checkout capabilities remain distinct linear facts; one sealed scheduler wave binds compatible roles to the exact Trusted-first per-owner Resolve/Verify rings and commits its cursor with the authority exchange. An immediately available completion cut settles and refills in one Apply without a fill timer; a finished capability never waits for a fair permit, while an idle coordinator may queue at most one fair acquisition. ResolveThenVerify continues from resolved evidence to VM work without an authority Apply only when the verifier capability permits the resolved cycle class; otherwise the same token settles to queued Verify. The exhaustive Apply compiler derives one coalesced compute prompt from role-compatible scheduler heads and active-work availability. The coordinator rechecks authority after every lossy prompt; signals carry no lane or owner authority. Ready and maintenance retain independent drivers and their documented ordering. | Effect saturation, allocation pressure, pause or stop, cancellation, completion reordering, a closed or full assignment transport, a stale scheduler cut, saturated peer or missing post-commit prompt must not lose a slot, fair permit, owner or completion, wait to fill a batch, spin or starve an eligible Resolve/Verify owner. Structural transport failure must return the exact checked-out capability. Strict Ready priority remains deterministic, batch-bounded and subject to Remote expiry. | T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14, T15, T16 | - Does every wait name an independent releaser?<br>- Can a worker or coordinator exit while retaining a slot, owner, fair permit or completion?<br>- Is queue fairness decided only from committed scheduler state, and is strict Ready priority kept separate?<br>- Does every runtime mutation consume one top-level post-commit receipt with no escaping control flow?<br>- Can any lossy prompt become lane, role or owner authority instead of triggering a complete bounded probe?<br>- Does runnable publication cover resource-eligibility changes even when the scheduler head identity is stable?<br>- Can a count grant be mistaken for worker-role or checkout authority, or can a cursor advance without the same Apply?<br>- Does every Finished slot have exactly one queued completion, and does shutdown consume that finite rank before joining the coordinator? | Resolve/verify remains parallel outside the authority guard, and a compatible ResolveThenVerify route adds no intermediate authority Apply or second capability. For A retained-ingress cuts, W compute waves and R Ready/effect slices, the independent common-path target is A + (W + 1) + 2R authority Applies. The coordinator adds one task, P one-slot assignment channels and one P-slot completion channel for retained slot bound P; it has no fill timer, retry queue or second scheduler. One coalesced compute prompt replaces role-specific publication, and each bounded probe derives exact work from the committed scheduler. |
| `TP-ADMIN-001` Cause-complete administration and peer revocation | `tx-pool/src/authority/plan.rs`: `enum AdminPlan`, `struct OwnerRemovalBatch`, `fn plan_administrative_removal`, `fn plan_peer_revocation`, `fn plan_local_removal`<br>`tx-pool/src/authority/ban.rs`: `struct PeerBanRegistry`, `struct PeerBanLease` | Clear, expiry, local removal and peer revocation use one cause-complete owner-removal compiler. Peer ban removes only not-yet-Accepted ingress owners; every delayed Remote message rejected by a retained peer fence commits its own exact relay release so another peer may refetch it. The fence registry has a hard count bound; saturation retires the oldest fence and sends any later delayed submission through complete bounded validation instead of blocking all Remote ingress. | Ban during compute, promoted remote ingress, delayed controller delivery after the revocation reset, accepted descendants, repeated ban, unbounded distinct-session churn, expiry, clear or local removal must not leave unbounded state, active work, dependency edges, charge or known-filter state behind; Accepted owners must not be removed by peer ban. | T1, T2, T3, T4, T6, T7, T8, T9, T10, T11, T12 | - Does each cause map all owner variants exhaustively?<br>- Are Accepted and not-yet-Accepted ban semantics separated by type?<br>- Does every remote removal publish refetch cleanliness exactly once?<br>- Is peer-fence saturation hard-bounded and limited to the documented oldest-session validation fallback? | Removal follows exact indexed cohorts and bounded descendant closure; the peer fence is fixed-size with amortized expiry/oldest eviction and no active-work drain, full scan or global Remote stop. |
| `TP-EFFECT-001` Atomic bounded effects and publication | `tx-pool/src/authority/effect.rs`: `enum CommittedEffect`, `struct EffectLog`, `enum EffectPolicy`, `struct EffectLimits`<br>`tx-pool/src/authority/runtime.rs`: `struct AuthorityEffectPublisherClaim`, `struct AuthorityEffectPublicationLease`, `fn wait_effect_publication`<br>`tx-pool/src/authority/publisher.rs`: `run_claimed_authority_effect_publisher`, `fn publish_committed_effect_batch`, `struct AuthorityEffectEndpoints` | Required callback, relay, reject, peer and parent-request outcomes are bounded immutable effects committed by the same Apply. Each record remains in one authoritative log location until one settlement Apply. The sole publisher borrows the minimum record through its lifetime-bound claim, performs I/O after the guard opens and settles progress without rereading ownership. Effectful compute settlements contending for the same capacity use their monotonic checkout identity as a well-founded no-overtake rank; unrelated no-effect settlements may bypass a capacity-blocked item. | Journal saturation, continuous newer completions, interposed append/close, relay disconnect, slow callback, reset replacement, publisher cancellation or endpoint retry must not starve an older same-capacity settlement, block unrelated no-effect progress, move a borrowed head, roll back state, replay completed endpoints, resurrect an older reset or duplicate publication authority. | T1, T2, T3, T4, T6, T7, T8, T9, T10 | - Does every public terminal outcome carry its effect in the same Plan?<br>- Can only rebuildable detail collapse to GenerationReset?<br>- Are region and indivisible-batch capacities valid at startup?<br>- Is receipt acquisition mutation-free and lifetime-bound to the sole publisher claim?<br>- Can a newer effectful completion repeatedly overtake an older waiter after capacity is released? | Publication is outside the authority guard; acquisition is one coherent read and settlement is the only Apply. Remote, Trusted and Critical capacity prevent hostile head blocking. |
| `TP-REORG-001` Reliable atomic chain reconciliation | `tx-pool/src/authority/chain_boundary.rs`: `struct ChainUpdateRequest`, `enum ChainPackaging`, `enum ChainBoundaryError`<br>`tx-pool/src/authority/plan/chain_transition.rs`: `fn plan_chain_transition`, `fn plan_chain_generation_replacement`, `fn select_chain_recoveries`<br>`tx-pool/src/authority/service.rs`: `fn run_ordered_chain_control_driver`<br>`tx-pool/src/service/message.rs`: `enum ChainControl`<br>`chain/src/verify.rs`: `install_chain_tip_transition` | One readiness-independent capacity-one control boundary pairs each installed snapshot with its exact fork delta and orders later generation clears after that reconciliation. One UAK Apply reconciles status, membership, recovery, dependencies, resources, effects and chain view. Resource-excluded new trusted recoveries are selected parent-first as a closed subtree exclusion while unrelated fitting roots remain eligible in both normal reconciliation and fresh-generation fallback; an already-owned PreAccepted descendant stays charged and re-enters validation under its source policy. | Blank-fork reorg, Gap outside the new window, detached dependency tree, short-ID collision, over-bound recovery, startup readiness false or truncate followed by clear must not lose a tip delta, let delayed recovery overtake clear, expose a partial owner generation or strand re-proposal. | T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13 | - Does every best-tip installation call the sole reliable boundary?<br>- Can ClearPool or ClearPipeline overtake an already-published chain transition?<br>- Can any derived retry replay authoritative chain mutation?<br>- Are recovered owners parent-first and ordinary validation inputs? | Only chain reconciliation and rare generation clears share the ordered lane; admission remains concurrent, packaging and sorting occur outside the guard, and derived template work remains separate. |
| `TP-PERSIST-001` Coherent bounded persistence | `tx-pool/src/authority/read.rs`: `fn capture_persistence`, `struct PersistenceReadReceipt`, `struct ParentFirstPersistence`<br>`tx-pool/src/authority/service.rs`: `fn save_pool`, `fn replay_persisted`, `fn shutdown`<br>`tx-pool/src/authority/topology.rs`: `fn shutdown`, `fn shutdown_authority`<br>`tx-pool/src/persisted.rs`: `struct PersistenceWriter`, `struct PersistenceLease`, `struct PersistenceSnapshot` | Persistence captures Accepted and Recovery-source owners from one authority read cut, releases the guard, orders parent-first, writes one bounded atomic snapshot and revalidates every replayed transaction. Startup derives one checked read ceiling and enforces it both before allocation and while the file is read. Final shutdown closes ingress, drains handlers and ordered control, joins authority workers, closes and drains committed effects, then joins derived tasks before capture; a derived-task failure is diagnostic, loss of an authority capability permanently forbids persistence, and external write failure is a distinct terminal best-effort outcome rather than false durability. | Save racing reorg/clear, recovery in any phase, malformed, growing or oversized file, configuration arithmetic overflow, duplicate partitions, writer failure or legacy v1 input must not persist an ownership splice, allocate unbounded data or trust stale verification evidence. | T1, T2, T4, T6, T7, T8, T9, T10, T11, T12 | - Are ReplacementHistory, Remote and Proposal owners excluded?<br>- Does sorting and file I/O occur after the authority guard opens?<br>- Are configured and observed file bounds checked before allocation, during reading and before fully validated replay?<br>- Can persistence begin before every authority capability and effect is settled, or can a derived-only failure erase a coherent cut?<br>- Can an external write failure be mistaken for either durable success or authority corruption? | Persistence clone/sort/I/O is off admission paths and serialized by one writer capability. |
| `TP-QUERY-001` Coherent public projections | `tx-pool/src/authority/read.rs`: `struct AuthorityReadView`, `enum AuthorityReadState`, `fn rpc_status_for_accepted`<br>`tx-pool/src/authority/query.rs`: `enum AuthorityTransactionLookup`, `enum AuthorityTransactionStatusLookup`, `fn transaction_status_lookup`, `struct PersistenceReceipt`<br>`tx-pool/src/service/dispatch.rs`: `fn handle_get_tx_status`, `fn handle_get_transaction_with_status` | RPC, compact-block, live-cell, pool detail, fee and persistence queries observe one coherent authority cut. Status and detailed transaction lookup are distinct read products: status performs no optional detail arithmetic, and an unrepresentable minimum replacement fee is `None` rather than authority invalidity, matching the compatibility boundary. Compatibility mapping never drives internal state. Full-pool queries use one bounded capture gate and reusable fallible scratch grown outside the authority guard with a finite resource-ledger-derived rank; the coherent cut only copies handles, while point queries, sorting and response construction remain independent of that cut. | Concurrent clear/reorg/admission, short-ID collision, ReplacementHistory lookup, optional replacement-fee overflow, storage delay, allocation failure or repeated small RPC queries must not splice generations, fabricate proof/status, invalidate a coherent authority generation, amplify without a bound or delay every Apply through O(pool) shared-lock work. | T1, T3, T5, T6, T7, T8, T9, T10, T12 | - Does each receipt own every value needed after the guard opens?<br>- Is ReplacementHistory hidden from the live RPC state?<br>- Can public Pending ever be reused as an internal phase decision?<br>- What are the exact query concurrency, response-residency and lock-hold bounds? | Coherent reads remain read-only. At most one full-pool capture scans the authority at once; allocation and sorting are outside the guard, point queries and template lanes are not serialized by the capture gate, and output-size compatibility cannot hide response-residency cost. |
| `TP-DEFECT-001` Rust-native ordinary failure boundary | `tx-pool/src/authority/service.rs`: `enum AuthorityServiceError`, `enum AuthorityIntegrityFault`, `fn settle_operation_error`, `enum AuthorityProjectionFault`<br>`tx-pool/src/authority/plan/ingress.rs`: `fn plan_retained_admission_batch`, `enum CommittedRetainedAdmissionBatch`<br>`tx-pool/src/authority/ingress.rs`: `enum RetainedIngressAttempt`, `enum RetainedIngressBoundaryError`<br>`tx-pool/src/authority/topology.rs`: `enum AuthorityGenerationFault`, `enum AuthorityDerivedTaskFailure`, `enum AuthorityShutdownStatus` | Legal, hostile, stale, duplicate, resource, cancellation and external or rebuildable-derived failures are typed local outcomes. Only a proven authority contradiction or loss of a sole linear capability can forbid persistence; panic-and-catch and broad fail-stop are not control flow. | Malformed transactions, allocation pressure, relay/cache/template failure, candidate-uncle source exhaustion, recent-reject encoding, task exit or endpoint timeout must not be mislabeled as peer rejection, silently accepted or escalated to service stop without a structural proof. | T1, T2, T3, T6, T7, T8, T9, T10, T11, T13 | - Can valid or hostile input construct any AuthorityFault or generation invalidity?<br>- Are derived failures isolated from transaction authority?<br>- Does production use panic-like operations as validation or retry logic? | Typed branches add no repair scan, restart loop or catch-unwind boundary to hot paths. |
| `TP-POOL-001` Atomic accepted membership graph | `tx-pool/src/authority/plan/membership.rs`: `struct MembershipProjection`, `struct PreparedMembership`, `struct ProjectionDelta`, `struct EvictionOrderKey`<br>`tx-pool/src/authority/plan/membership/eviction.rs`: `fn complete_removals`, `fn apply_removals`, `fn apply_candidate` | Accepted ownership, spender relation, causal parents, aggregates, status counts and eviction order are one derived MembershipProjection changed atomically with owners and resources. Capacity chooses the smallest exact tuple of status, max(self, descendant) integer fee rate over CKB size/cycle weight, descendant count, arrival and raw identity, then removes one complete valid descendant closure. | Diamond/fan-in graph, late parent, conditional reader/spender, capacity eviction, status change or RBF descendant removal must not leave a surviving invalid consumer, stale aggregate or partial component. | T1, T3, T4, T5, T6, T8, T9, T10, T11, T12, T13 | - Does one canonical graph delta update every affected aggregate once?<br>- Are inputs and cell-dep reader relations intentionally distinct?<br>- Does capacity remove a complete valid component and never a candidate ancestor?<br>- Does every capacity comparison preserve CKB weight, integer rounding and every deterministic tie field? | Sparse bounded graph deltas replace full-pool rebuilds; deterministic order uses checked integer comparison. |
| `TP-TEMPLATE-001` Concurrent versioned template convergence | `tx-pool/src/authority/template.rs`: `struct TemplateConvergence`, `struct AuthorityTemplateReadReceipt`, `enum TemplatePublication`<br>`tx-pool/src/authority/template_driver.rs`: `struct AuthorityBlockAssembler`, `fn run_replacement_lane`, `fn run_component_lane`<br>`tx-pool/src/authority/packing.rs`: `struct TemplatePackingLimits` | One ordered full/reset lane and optimistic proposal, transaction and uncle lanes build from immutable receipts and publish only against current chain/source/template versions. The block assembler remains derived and rebuildable. | Recovered Gap tree, detached uncle proposal overlap, stale reset/full, conditional cycle, CPFP fan-in, optional byte pressure or rebuild failure must not publish invalid order, censor re-proposal, overwrite newer output or spin. | T1, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13 | - Are full/reset serialized without serializing partial/uncle construction?<br>- Does every lane publish only an exact current receipt?<br>- Are optional proposals/uncles packed under one byte budget using only published conflicts? | Builds run outside authority/publication guards; O(1) source probes skip irrelevant population capture. |
| `TP-IDENTITY-001` Exact transaction and evidence identity | `tx-pool/src/authority/state.rs`: `struct TxIdentity`, `struct RawTxHash`, `struct WitnessTxHash`, `struct ProposalId`<br>`tx-pool/src/authority/chain.rs`: `struct CellLocationReceipt`, `struct VerificationContextReceipt`<br>`verification/src/cache.rs`: `struct TxVerificationCacheKey`, `enum ScriptVerificationRules`<br>`tx-pool/src/authority/validation.rs`: `struct FinalAdmissionValidation`, `fn validate_membership` | Ownership uses raw hash, script-cache reuse uses inline witness hash plus script rules, proposal short ID is collision-aware protocol indexing, and chain evidence is bound to an exact snapshot/view and per-cell provenance. | Witness variant, short-ID collision, nearby cache request, hardfork rule change, mixed snapshot, same-tip unproven cell or chain-view ABA must not reuse another transaction or context's proof. | T1, T2, T3, T4, T6, T7, T8, T9, T10, T11 | - Can a cache caller omit either witness identity or script-rule generation?<br>- Can a short ID decide ownership or duplicate identity?<br>- Is every positive evidence reuse per-input and tx-pool-only? | The cache key is a copy-cheap fixed array plus small enum; exact evidence reuse avoids redundant same-tip chain reads without a new cache. |
| `TP-PERF-001` Bounded work and preserved concurrency | `tx-pool/src/authority/runtime.rs`: `struct AuthorityStoreLock`, `fn execute_resolution`, `fn execute_verification`, `fn try_drive_ready`<br>`tx-pool/src/authority/publisher.rs`: `fn publish_committed_effect_batch`<br>`tx-pool/src/authority/scheduler.rs`: `struct FairFrontier`, `struct ReadyKey`<br>`tx-pool/src/authority/relay.rs`: `struct AuthorityRelaySink`, `struct AuthorityRelayReceiver`, `fn production_authority_relay_mailbox`<br>`tx-pool/src/block_assembler/candidate_uncles.rs`: `struct CandidateUncles`<br>`tx-pool/src/service/builder.rs`: `fn run`, `fn run_dispatcher` | Peer-controlled count, bytes, edges, fanout, closure, probes, channels and candidate sets are bounded while independent resolution/verification and optimistic template work remain concurrent and deterministic. Immediately available homogeneous independent ingress, compute completion/checkout and Ready work share bounded canonical Plan/Apply cuts without a timer; fixed-width batching is claimed as serial-cut amortization, never as a false asymptotic improvement. | Idle-peer cardinality, saturated owner, long conditional graph, mailbox overflow, full candidate-uncle set, extreme fee weights or controller pressure must not cause unbounded scan, overflow, global serialization or nondeterministic order. | T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14, T15, T16 | - Is every hostile loop bounded by a named constant or charged set?<br>- Does an optimization preserve the one authority and exact validation?<br>- Has every statically inferable complexity issue closed before profiling?<br>- Does one barrier-released worker wave share a serial Apply cut without increasing prompt single-item latency? | No optimization claim is accepted without operation-count evidence and fixed-binary A/B after correctness gates. |
| `TP-HANDOFF-001` Bounded controller and relay handoff conservation | `tx-pool/src/service.rs`: `DEFAULT_CHANNEL_SIZE`, `CHAIN_CONTROL_CHANNEL_SIZE`, `struct AdministrationGate`, `struct AdminAdmission`, `struct AdmittedAdministration`, `struct Request`, `fn respond`<br>`tx-pool/src/service/builder.rs`: `enum RetainedIngressBatch`, `fn run_dispatcher`, `handler_limit`, `JoinSet`<br>`tx-pool/src/service/controller.rs`: `struct TxPoolController`, `send_message`, `send_admitted_chain_control`<br>`tx-pool/src/service/dispatch.rs`: `pub(crate) async fn process`, `fn process_retained_ingress_batch`, `fn settle_remote_responder_prefix`, `fn respond_outer`<br>`tx-pool/src/authority/plan/ingress.rs`: `struct PreparedRetainedAdmissionBatch`, `enum CommittedRetainedAdmissionBatch`, `fn plan_retained_admission_batch`<br>`tx-pool/src/authority/service.rs`: `fn submit_remote_batch`, `fn submit_proposal_batch`, `fn run_ordered_chain_control_driver`<br>`sync/src/relayer/transactions_process.rs`: `struct TransactionsProcess`<br>`sync/src/relayer/block_proposal_process.rs`: `struct BlockProposalProcess`<br>`sync/src/types/mod.rs`: `fn mark_as_known_txs`, `fn remove_inflight_proposals`, `fn record_accepted_tx` | Every controller command payload occupies exactly one caller, waiting sender, queued, handler-owned or terminal protocol location. Enqueue acknowledgement and optional sender/receiver response states are separate observations, so a no-responder notification may return accepted while its payload remains queued. Remote pre-admission known state is released by the exact failed request or a committed terminal effect. A Proposal response consumes its requested in-flight row exactly once but remains unknown until a committed Accepted effect; replay and controller failure therefore create neither duplicate work nor a stale known mark. Ordinary request count and handler concurrency are bounded. Lossless reorg publication has one trusted producer; public clear operations share one non-cloneable administrative admission capability, so one accepted clear preserves order and excess calls fail fast instead of multiplying waiting payloads. Queue-full, closure, cancellation and response abandonment cannot create authority ownership or retroactively decide whether a committed transition happened. | Queue saturation, delayed handling, caller cancellation, dropped response receivers, callback reentrancy, shutdown or a full ordered lane must not duplicate a request, leak its retained payload, block unrelated dispatcher progress, let an RPC clear flood multiply unbounded waiting sends or suppress chain reconciliation, suppress an already committed authority result, or leave stale relay state that prevents the same raw transaction from being fetched from another peer. | T1, T2, T3, T6, T7, T8, T9, T10, T11, T12, T16 | - Does every payload move between ownership locations exactly once while acknowledgement remains a separate fact?<br>- Are request count, payload bytes, handler concurrency and response residency bounded separately?<br>- Can responder loss influence authority mutation or only result delivery?<br>- Are ordinary concurrent requests kept separate from ordered chain controls?<br>- What bounds producer-owned payloads suspended before a reliable ordered send? | The ordinary queue retains at most DEFAULT_CHANNEL_SIZE requests and the dispatcher owns at most its checked handler limit. The ordered lane retains at most its channel, active command, one trusted reorg sender and one admitted administrative sender; excess public clears fail before waiting. Relay settlement follows only the acknowledged request or exact terminal effect and adds no pool scan. |

### Executable evidence

#### `TP-OWN-001` - Single transaction ownership and ABA safety

Generated focused command: `cargo nextest run -p ckb-tx-pool --features internal -E 'test(=authority::tests::compute_exchange::uak_compute_exchange_rejects_an_old_completion_while_replacement_is_computing) | test(=authority::tests::foundation::uak_active_trusted_witness_replacement_atomically_stales_obsolete_work) | test(=authority::tests::foundation::uak_all_four_preaccepted_phases_are_closed_variants) | test(=authority::tests::foundation::uak_direct_local_admission_moves_from_absent_to_accepted_in_one_apply) | test(=authority::tests::foundation::uak_duplicate_and_promotion_never_create_second_owner) | test(=authority::tests::foundation::uak_stale_compute_version_is_mutation_free_across_aba) | test(=authority::tests::settlement_refinement::uak_settlement_classifier_refines_every_legal_result_staleness_and_policy_cut) | test(=authority::tests::settlement_refinement::uak_settlement_reference_retires_resource_rejection_after_baseline_loss) | test(=mathematical_model::properties::model_invariant_basis_rejects_removed_resource_version_and_effect_premises) | test(=mathematical_model::properties::model_invariant_detects_a_computing_owner_without_its_capability) | test(=mathematical_model::properties::model_sequential_lifecycle_preserves_owner_charge_capability_and_effect_laws) | test(=mathematical_model::properties::model_stale_completion_retires_only_its_linear_capability) | test(=mathematical_model::settlement_transition_properties::model_every_settlement_result_requires_the_active_baseline_to_remain_current) | test(=mathematical_model::settlement_transition_properties::model_queued_verify_requires_exact_identity_view_cut_chain_and_dependencies) | test(=mathematical_model::settlement_transition_properties::model_ready_requires_both_payload_identity_and_sealed_witness) | test(=mathematical_model::settlement_transition_properties::model_rejection_validity_is_chain_or_resource_bound) | test(=mathematical_model::settlement_transition_properties::model_verification_rejection_policy_transition_is_a_closed_truth_table) | test(=mathematical_model::settlement_transition_properties::model_waiting_is_retained_only_at_the_same_chain_and_current_missing_cut)'`

Rust evidence:

- `authority::tests::compute_exchange::uak_compute_exchange_rejects_an_old_completion_while_replacement_is_computing` (T1, T2, T3, T6, T10, T11)
- `authority::tests::foundation::uak_active_trusted_witness_replacement_atomically_stales_obsolete_work` (T1, T2, T11)
- `authority::tests::foundation::uak_all_four_preaccepted_phases_are_closed_variants` (T1, T5)
- `authority::tests::foundation::uak_direct_local_admission_moves_from_absent_to_accepted_in_one_apply` (T1, T3, T6, T7)
- `authority::tests::foundation::uak_duplicate_and_promotion_never_create_second_owner` (T1, T2, T3)
- `authority::tests::foundation::uak_stale_compute_version_is_mutation_free_across_aba` (T1, T2, T6)
- `authority::tests::settlement_refinement::uak_settlement_classifier_refines_every_legal_result_staleness_and_policy_cut` (T1, T2, T4, T6, T10, T11)
- `authority::tests::settlement_refinement::uak_settlement_reference_retires_resource_rejection_after_baseline_loss` (T1, T2, T4, T6, T10, T11)
- `mathematical_model::properties::model_invariant_basis_rejects_removed_resource_version_and_effect_premises` (T1, T2, T3, T4, T6, T7, T8, T11)
- `mathematical_model::properties::model_invariant_detects_a_computing_owner_without_its_capability` (T2, T5)
- `mathematical_model::properties::model_sequential_lifecycle_preserves_owner_charge_capability_and_effect_laws` (T1, T2, T3, T5, T6, T7)
- `mathematical_model::properties::model_stale_completion_retires_only_its_linear_capability` (T1, T2, T6)
- `mathematical_model::settlement_transition_properties::model_every_settlement_result_requires_the_active_baseline_to_remain_current` (T1, T2, T4, T6, T10, T11)
- `mathematical_model::settlement_transition_properties::model_queued_verify_requires_exact_identity_view_cut_chain_and_dependencies` (T1, T2, T4, T6, T10, T11)
- `mathematical_model::settlement_transition_properties::model_ready_requires_both_payload_identity_and_sealed_witness` (T1, T2, T4, T6, T10, T11)
- `mathematical_model::settlement_transition_properties::model_rejection_validity_is_chain_or_resource_bound` (T1, T2, T4, T6, T10, T11)
- `mathematical_model::settlement_transition_properties::model_verification_rejection_policy_transition_is_a_closed_truth_table` (T1, T2, T4, T6, T10, T11)
- `mathematical_model::settlement_transition_properties::model_waiting_is_retained_only_at_the_same_chain_and_current_missing_cut` (T1, T2, T4, T6, T10, T11)

Cross-crate Rust evidence:

Generated cross-crate command: `cargo nextest run -p ckb-rpc -E 'test(=tests::examples::test_rpc_examples)'`

- `rpc/src/tests/examples.rs::tests::examples::test_rpc_examples` (T1, T6, T10) - The local-test RPC starts from an absent transaction and observes the final direct admission result instead of relying on the retired verify-queue acknowledgement gap.

Process-level evidence:

- `local-test-rpc-direct`: `test/src/specs/tx_pool/local_test_submission.rs::LocalTestSubmissionIsDirect` (T1, T2, T3, T6, T7, T8, T11) - Local admission remains direct and atomic while TestAccept remains read-only under the same policy. Paired units: `authority::tests::foundation::uak_direct_local_admission_moves_from_absent_to_accepted_in_one_apply`, `authority::tests::chain::uak_final_admission_receipt_is_stale_after_chain_view_aba`, `authority::tests::resolver::uak_duplicate_inputs_are_a_malformed_outcome_not_an_authority_fault`. Command: `make integration CKB_TEST_ARGS='-c 1 LocalTestSubmissionIsDirect'`

#### `TP-COMMIT-001` - Read-only Plan and total Apply

Generated focused command: `cargo nextest run -p ckb-tx-pool --features internal -E 'test(=authority::tests::atomic_refinement::uak_apply_clock_reservation_adopts_only_model_legal_owner_progress) | test(=authority::tests::atomic_refinement::uak_apply_clock_reservation_refines_every_finite_model_demand) | test(=authority::tests::atomic_refinement::uak_apply_clock_reservation_refines_model_counter_boundaries) | test(=authority::tests::atomic_refinement::uak_discardable_clock_plan_refines_owner_demand_before_apply_sealing) | test(=authority::tests::chain::uak_final_admission_receipt_is_stale_after_chain_view_aba) | test(=authority::tests::foundation::uak_dropped_prepared_apply_is_semantically_mutation_free) | test(=authority::tests::foundation::uak_independent_batch_reserves_one_apply_sequence_and_distinct_versions) | test(=authority::tests::foundation::uak_independent_plan_drop_and_batch_clock_failure_are_mutation_free) | test(=authority::tests::foundation::uak_independent_run_matches_every_canonical_single_prefix) | test(=authority::tests::foundation::uak_terminal_outcome_and_effect_commit_together) | test(=authority::tests::ingress_batch::uak_retained_ingress_batch_noop_has_no_apply_or_clock_advance) | test(=authority::tests::ingress_batch::uak_retained_ingress_batch_observes_prior_items_in_canonical_order) | test(=authority::tests::ingress_batch::uak_retained_ingress_batch_refines_the_canonical_proposal_fold) | test(=authority::tests::refinement::uak_candidate_accepted_role_product_refines_the_executable_model_pointwise) | test(=authority::tests::refinement::uak_candidate_role_product_refines_the_executable_model_pointwise) | test(=authority::tests::refinement::uak_every_four_owner_coupling_graph_refines_the_model_prefix) | test(=authority::tests::refinement::uak_every_single_coupled_edge_position_refines_the_model_prefix) | test(=authority::tests::refinement::uak_ready_economic_order_refines_when_fee_and_rate_disagree) | test(=authority::tests::refinement::uak_source_control_classes_refine_the_model_prefix) | test(=authority::tests::refinement::uak_stale_ready_evidence_refines_the_model_terminal) | test(=authority::tests::validation::uak_direct_validation_shares_the_final_validator_without_mutation_authority) | test(=mathematical_model::adversarial_properties::model_generated_cell_shapes_route_only_the_exact_independent_prefix) | test(=mathematical_model::adversarial_properties::model_independent_hostile_trace_search_preserves_every_commit_contract) | test(=mathematical_model::atomic_transition_properties::model_batch_clock_commit_uses_one_apply_sequence_for_every_member) | test(=mathematical_model::atomic_transition_properties::model_clock_commit_advances_one_sequence_and_the_exact_identity_ranges) | test(=mathematical_model::atomic_transition_properties::model_clock_commit_is_total_or_mutation_free_at_every_counter_boundary) | test(=mathematical_model::atomic_transition_properties::model_discardable_clock_plan_does_not_require_apply_sequence_capacity) | test(=mathematical_model::atomic_transition_properties::model_transition_controls_make_every_required_projection_structural) | test(=mathematical_model::composition_properties::model_batch_plan_rejects_a_changed_authority_cut_without_mutation) | test(=mathematical_model::composition_properties::model_batch_reserves_one_apply_stamp_even_at_the_counter_boundary) | test(=mathematical_model::composition_properties::model_dynamic_footprint_keeps_reads_writes_headers_and_pool_origin_distinct) | test(=mathematical_model::composition_properties::model_ordered_batch_refuses_an_unowned_transition_family) | test(=mathematical_model::composition_properties::model_ordered_ingress_batch_is_one_apply_and_matches_canonical_submission) | test(=mathematical_model::composition_properties::model_ready_batch_equals_the_canonical_no_interleave_fold_with_one_stamp) | test(=mathematical_model::properties::model_chain_revision_prevents_view_hash_aba_from_reviving_old_work) | test(=mathematical_model::properties::model_cold_retained_lifecycle_exposes_the_exact_sequential_apply_cost) | test(=mathematical_model::properties::model_deterministic_replay_produces_identical_states_and_dispositions) | test(=mathematical_model::properties::model_kernel_sequence_helper_rejects_no_valid_transition) | test(=mathematical_model::properties::model_kernel_step_is_total_and_preserves_invariants_for_bounded_traces) | test(=mathematical_model::properties::model_local_direct_success_uses_no_retained_owner_before_its_single_final_apply) | test(=mathematical_model::properties::model_multi_owner_apply_uses_one_stamp_distinct_versions_and_canonical_effect_order) | test(=mathematical_model::properties::model_ready_capture_commits_the_unchanged_strict_priority_prefix_with_one_stamp) | test(=mathematical_model::properties::model_test_accept_and_local_share_bounded_resource_exclusion) | test(=mathematical_model::properties::model_test_accept_duplicate_is_read_only_and_local_duplicate_is_acknowledged) | test(=mathematical_model::properties::model_test_accept_observes_the_same_success_policy_without_authority_mutation) | test(=mathematical_model::properties::model_total_system_step_preserves_invariants_for_bounded_traces) | test(=mathematical_model::refinement::tests::adding_candidate_relations_never_extends_the_independent_prefix) | test(=mathematical_model::refinement::tests::finite_role_products_and_graph_masks_are_total)'`

Rust evidence:

- `authority::tests::atomic_refinement::uak_apply_clock_reservation_adopts_only_model_legal_owner_progress` (T1, T2, T3, T6, T7)
- `authority::tests::atomic_refinement::uak_apply_clock_reservation_refines_every_finite_model_demand` (T1, T2, T3, T6, T7)
- `authority::tests::atomic_refinement::uak_apply_clock_reservation_refines_model_counter_boundaries` (T1, T2, T3, T6, T7)
- `authority::tests::atomic_refinement::uak_discardable_clock_plan_refines_owner_demand_before_apply_sealing` (T1, T2, T3, T6, T7)
- `authority::tests::chain::uak_final_admission_receipt_is_stale_after_chain_view_aba` (T2, T6, T9, T11)
- `authority::tests::foundation::uak_dropped_prepared_apply_is_semantically_mutation_free` (T6)
- `authority::tests::foundation::uak_independent_batch_reserves_one_apply_sequence_and_distinct_versions` (T2, T6, T8, T14)
- `authority::tests::foundation::uak_independent_plan_drop_and_batch_clock_failure_are_mutation_free` (T2, T3, T5, T6)
- `authority::tests::foundation::uak_independent_run_matches_every_canonical_single_prefix` (T3, T4, T5, T6)
- `authority::tests::foundation::uak_terminal_outcome_and_effect_commit_together` (T6, T7)
- `authority::tests::ingress_batch::uak_retained_ingress_batch_noop_has_no_apply_or_clock_advance` (T2, T6)
- `authority::tests::ingress_batch::uak_retained_ingress_batch_observes_prior_items_in_canonical_order` (T1, T3, T6, T14)
- `authority::tests::ingress_batch::uak_retained_ingress_batch_refines_the_canonical_proposal_fold` (T1, T3, T6, T14)
- `authority::tests::refinement::uak_candidate_accepted_role_product_refines_the_executable_model_pointwise` (T1, T3, T4, T5, T6, T8, T11)
- `authority::tests::refinement::uak_candidate_role_product_refines_the_executable_model_pointwise` (T4, T5, T6, T11)
- `authority::tests::refinement::uak_every_four_owner_coupling_graph_refines_the_model_prefix` (T4, T5, T6, T8)
- `authority::tests::refinement::uak_every_single_coupled_edge_position_refines_the_model_prefix` (T4, T5, T6, T8)
- `authority::tests::refinement::uak_ready_economic_order_refines_when_fee_and_rate_disagree` (T5, T6, T8)
- `authority::tests::refinement::uak_source_control_classes_refine_the_model_prefix` (T3, T5, T6, T8)
- `authority::tests::refinement::uak_stale_ready_evidence_refines_the_model_terminal` (T2, T6, T9, T11)
- `authority::tests::validation::uak_direct_validation_shares_the_final_validator_without_mutation_authority` (T1, T2, T6, T11)
- `mathematical_model::adversarial_properties::model_generated_cell_shapes_route_only_the_exact_independent_prefix` (T1, T3, T4, T5, T6, T8, T11)
- `mathematical_model::adversarial_properties::model_independent_hostile_trace_search_preserves_every_commit_contract` (T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11)
- `mathematical_model::atomic_transition_properties::model_batch_clock_commit_uses_one_apply_sequence_for_every_member` (T1, T2, T3, T6, T7)
- `mathematical_model::atomic_transition_properties::model_clock_commit_advances_one_sequence_and_the_exact_identity_ranges` (T1, T2, T3, T6, T7)
- `mathematical_model::atomic_transition_properties::model_clock_commit_is_total_or_mutation_free_at_every_counter_boundary` (T1, T2, T3, T6, T7)
- `mathematical_model::atomic_transition_properties::model_discardable_clock_plan_does_not_require_apply_sequence_capacity` (T1, T2, T3, T6, T7)
- `mathematical_model::atomic_transition_properties::model_transition_controls_make_every_required_projection_structural` (T1, T2, T3, T6, T7)
- `mathematical_model::composition_properties::model_batch_plan_rejects_a_changed_authority_cut_without_mutation` (T2, T6, T11)
- `mathematical_model::composition_properties::model_batch_reserves_one_apply_stamp_even_at_the_counter_boundary` (T2, T6, T8)
- `mathematical_model::composition_properties::model_dynamic_footprint_keeps_reads_writes_headers_and_pool_origin_distinct` (T1, T3, T4, T5, T6, T8, T11)
- `mathematical_model::composition_properties::model_ordered_batch_refuses_an_unowned_transition_family` (T6)
- `mathematical_model::composition_properties::model_ordered_ingress_batch_is_one_apply_and_matches_canonical_submission` (T1, T3, T6, T8, T14, T16)
- `mathematical_model::composition_properties::model_ready_batch_equals_the_canonical_no_interleave_fold_with_one_stamp` (T1, T3, T4, T5, T6, T7, T8, T11, T14)
- `mathematical_model::properties::model_chain_revision_prevents_view_hash_aba_from_reviving_old_work` (T2, T6, T9, T11)
- `mathematical_model::properties::model_cold_retained_lifecycle_exposes_the_exact_sequential_apply_cost` (T2, T5, T6, T7, T8)
- `mathematical_model::properties::model_deterministic_replay_produces_identical_states_and_dispositions` (T6, T9, T11)
- `mathematical_model::properties::model_kernel_sequence_helper_rejects_no_valid_transition` (T6)
- `mathematical_model::properties::model_kernel_step_is_total_and_preserves_invariants_for_bounded_traces` (T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11)
- `mathematical_model::properties::model_local_direct_success_uses_no_retained_owner_before_its_single_final_apply` (T1, T2, T3, T6, T7)
- `mathematical_model::properties::model_multi_owner_apply_uses_one_stamp_distinct_versions_and_canonical_effect_order` (T1, T2, T3, T6, T7)
- `mathematical_model::properties::model_ready_capture_commits_the_unchanged_strict_priority_prefix_with_one_stamp` (T2, T5, T6, T10)
- `mathematical_model::properties::model_test_accept_and_local_share_bounded_resource_exclusion` (T3, T6, T8)
- `mathematical_model::properties::model_test_accept_duplicate_is_read_only_and_local_duplicate_is_acknowledged` (T1, T6, T7, T12)
- `mathematical_model::properties::model_test_accept_observes_the_same_success_policy_without_authority_mutation` (T1, T3, T6, T7, T12)
- `mathematical_model::properties::model_total_system_step_preserves_invariants_for_bounded_traces` (T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13)
- `mathematical_model::refinement::tests::adding_candidate_relations_never_extends_the_independent_prefix` (T4, T5, T6)
- `mathematical_model::refinement::tests::finite_role_products_and_graph_masks_are_total` (T4, T5, T6, T8, T11)

Cross-crate Rust evidence:

Generated cross-crate command: `cargo nextest run -p ckb-rpc -E 'test(=tests::examples::test_rpc_examples)'`

- `rpc/src/tests/examples.rs::tests::examples::test_rpc_examples` (T1, T6, T10) - The local-test RPC starts from an absent transaction and observes the final direct admission result instead of relying on the retired verify-queue acknowledgement gap.

Process-level evidence:

- `failed-rbf-terminal`: `test/src/specs/tx_pool/replace.rs::RbfContainInvalidInput` (T1, T3, T6, T7, T8, T12) - A policy-rejected RBF candidate publishes per-hash rejection while preserving every accepted owner and creating no uncharged conflict-history residency. Paired units: `authority::tests::foundation::uak_terminal_outcome_and_effect_commit_together`, `authority::tests::foundation::uak_failed_rbf_fee_disposition_preserves_victims_and_terminalizes_candidate`, `authority::tests::foundation::uak_independent_rbf_churn_never_exceeds_replacement_history_budget`, `authority::tests::query::uak_owned_transaction_query_hides_replacement_history_and_reports_minimum_fee`. Command: `make integration CKB_TEST_ARGS='-c 1 RbfContainInvalidInput'`
- `local-test-rpc-direct`: `test/src/specs/tx_pool/local_test_submission.rs::LocalTestSubmissionIsDirect` (T1, T2, T3, T6, T7, T8, T11) - Local admission remains direct and atomic while TestAccept remains read-only under the same policy. Paired units: `authority::tests::foundation::uak_direct_local_admission_moves_from_absent_to_accepted_in_one_apply`, `authority::tests::chain::uak_final_admission_receipt_is_stale_after_chain_view_aba`, `authority::tests::resolver::uak_duplicate_inputs_are_a_malformed_outcome_not_an_authority_fault`. Command: `make integration CKB_TEST_ARGS='-c 1 LocalTestSubmissionIsDirect'`

#### `TP-RBF-001` - Atomic deterministic replacement

Generated focused command: `cargo nextest run -p ckb-tx-pool --features internal -E 'test(=authority::tests::foundation::uak_failed_rbf_fee_disposition_preserves_victims_and_terminalizes_candidate) | test(=authority::tests::foundation::uak_rbf_accepts_new_input_only_with_positive_chain_evidence) | test(=authority::tests::foundation::uak_rbf_component_bound_stops_before_any_authority_mutation) | test(=authority::tests::foundation::uak_rbf_dependency_on_any_victim_is_mutation_free) | test(=authority::tests::foundation::uak_rbf_replaces_the_complete_descendant_closure_atomically) | test(=authority::tests::released_input_refinement::uak_chain_backed_input_survives_without_a_pool_parent_in_both_removal_modes) | test(=authority::tests::released_input_refinement::uak_replacement_and_administration_share_the_final_owner_projection) | test(=mathematical_model::adversarial_properties::model_rbf_is_coupled_to_the_accepted_victim_and_never_uses_the_independent_lane) | test(=mathematical_model::develop_refinement::develop_failed_rbf_can_remove_the_victim_before_candidate_retention) | test(=mathematical_model::evidence_transition_properties::model_released_input_is_derived_from_the_projected_final_owner_set) | test(=mathematical_model::evidence_transition_properties::model_replacement_history_trigger_is_exactly_conflict_or_removed_pool_producer) | test(=mathematical_model::properties::model_failed_rbf_is_terminal_and_never_mutates_the_victim) | test(=mathematical_model::properties::model_rbf_and_capacity_share_one_apply_without_collapsing_removal_causes) | test(=mathematical_model::properties::model_rbf_apply_terminalizes_trusted_victim_dependents_in_the_same_effect_batch) | test(=mathematical_model::properties::model_rbf_fee_floor_and_new_unconfirmed_input_match_policy) | test(=mathematical_model::properties::model_successful_rbf_moves_the_complete_victim_set_to_history_and_recovers_it) | test(=mathematical_model::properties::model_test_accept_and_local_share_the_exact_rbf_rejection_policy)'`

Rust evidence:

- `authority::tests::foundation::uak_failed_rbf_fee_disposition_preserves_victims_and_terminalizes_candidate` (T1, T3, T6, T7)
- `authority::tests::foundation::uak_rbf_accepts_new_input_only_with_positive_chain_evidence` (T6, T11)
- `authority::tests::foundation::uak_rbf_component_bound_stops_before_any_authority_mutation` (T6, T8)
- `authority::tests::foundation::uak_rbf_dependency_on_any_victim_is_mutation_free` (T4, T6, T11)
- `authority::tests::foundation::uak_rbf_replaces_the_complete_descendant_closure_atomically` (T1, T3, T4, T6)
- `authority::tests::released_input_refinement::uak_chain_backed_input_survives_without_a_pool_parent_in_both_removal_modes` (T1, T3, T4, T6, T8, T10)
- `authority::tests::released_input_refinement::uak_replacement_and_administration_share_the_final_owner_projection` (T1, T3, T4, T6, T8, T10)
- `mathematical_model::adversarial_properties::model_rbf_is_coupled_to_the_accepted_victim_and_never_uses_the_independent_lane` (T1, T3, T4, T6, T8)
- `mathematical_model::develop_refinement::develop_failed_rbf_can_remove_the_victim_before_candidate_retention` (T1, T3, T6, T7)
- `mathematical_model::evidence_transition_properties::model_released_input_is_derived_from_the_projected_final_owner_set` (T1, T3, T4, T6, T8, T10)
- `mathematical_model::evidence_transition_properties::model_replacement_history_trigger_is_exactly_conflict_or_removed_pool_producer` (T1, T3, T4, T6, T8, T10)
- `mathematical_model::properties::model_failed_rbf_is_terminal_and_never_mutates_the_victim` (T1, T3, T4, T6, T7)
- `mathematical_model::properties::model_rbf_and_capacity_share_one_apply_without_collapsing_removal_causes` (T1, T3, T4, T6, T7, T8)
- `mathematical_model::properties::model_rbf_apply_terminalizes_trusted_victim_dependents_in_the_same_effect_batch` (T1, T2, T3, T4, T6, T7, T10)
- `mathematical_model::properties::model_rbf_fee_floor_and_new_unconfirmed_input_match_policy` (T1, T3, T4, T6, T7, T11)
- `mathematical_model::properties::model_successful_rbf_moves_the_complete_victim_set_to_history_and_recovers_it` (T1, T3, T4, T6, T7, T10)
- `mathematical_model::properties::model_test_accept_and_local_share_the_exact_rbf_rejection_policy` (T1, T3, T4, T6, T7, T12)

Process-level evidence:

- `failed-rbf-terminal`: `test/src/specs/tx_pool/replace.rs::RbfContainInvalidInput` (T1, T3, T6, T7, T8, T12) - A policy-rejected RBF candidate publishes per-hash rejection while preserving every accepted owner and creating no uncharged conflict-history residency. Paired units: `authority::tests::foundation::uak_terminal_outcome_and_effect_commit_together`, `authority::tests::foundation::uak_failed_rbf_fee_disposition_preserves_victims_and_terminalizes_candidate`, `authority::tests::foundation::uak_independent_rbf_churn_never_exceeds_replacement_history_budget`, `authority::tests::query::uak_owned_transaction_query_hides_replacement_history_and_reports_minimum_fee`. Command: `make integration CKB_TEST_ARGS='-c 1 RbfContainInvalidInput'`
- `rbf-basic`: `test/src/specs/tx_pool/replace.rs::RbfBasic` (T1, T3, T4, T6, T7, T10) - A valid replacement atomically installs the winner and bounded recovery disposition. Paired units: `authority::tests::foundation::uak_replacement_history_wakes_only_on_newer_projected_availability`, `authority::tests::foundation::uak_rbf_replaces_the_complete_descendant_closure_atomically`, `authority::tests::foundation::uak_failed_rbf_fee_disposition_preserves_victims_and_terminalizes_candidate`. Command: `make integration CKB_TEST_ARGS='-c 1 RbfBasic'`
- `rbf-cell-deps`: `test/src/specs/tx_pool/replace.rs::RbfCellDepsCheck` (T1, T4, T6, T11, T12) - A replacement depending on any victim is rejected before graph mutation. Paired units: `authority::tests::foundation::uak_rbf_dependency_on_any_victim_is_mutation_free`, `authority::tests::foundation::uak_membership_projects_one_spender_and_one_causal_graph`. Command: `make integration CKB_TEST_ARGS='-c 1 RbfCellDepsCheck'`
- `rbf-concurrency`: `test/src/specs/tx_pool/replace.rs::RbfConcurrency` (T1, T2, T6, T10) - Concurrent replacements preserve one highest-fee winner; direct policy losers terminalize, while only contenders actually displaced from Accepted enter charged replacement history. Paired units: `authority::tests::foundation::uak_replacement_history_wakes_only_on_newer_projected_availability`, `authority::tests::foundation::uak_failed_rbf_fee_disposition_preserves_victims_and_terminalizes_candidate`. Command: `make integration CKB_TEST_ARGS='-c 1 RbfConcurrency'`
- `rbf-conflict-recovery`: `test/src/specs/tx_pool/orphan_tx_recovery.rs::RbfOrphanRecovery` (T1, T3, T4, T6, T7, T9, T10) - Replacement history recovers only after a newer exact blocker release and re-enters validation. Paired units: `authority::tests::chain::uak_replacement_history_survives_winner_commit_and_wakes_after_reorg`, `authority::tests::foundation::uak_failed_rbf_fee_disposition_preserves_victims_and_terminalizes_candidate`. Command: `make integration CKB_TEST_ARGS='-c 1 RbfOrphanRecovery'`
- `rbf-cycling-attack`: `test/src/specs/tx_pool/replace.rs::RbfCyclingAttack` (T1, T2, T3, T4, T6, T8, T10) - Replacement cycling stays mutation-safe, bounded and unable to self-wake retained history. Paired units: `authority::tests::foundation::uak_replacement_history_wakes_only_on_newer_projected_availability`, `authority::tests::foundation::uak_independent_rbf_churn_never_exceeds_replacement_history_budget`, `authority::tests::foundation::uak_rbf_replaces_the_complete_descendant_closure_atomically`. Command: `make integration CKB_TEST_ARGS='-c 1 RbfCyclingAttack'`
- `rbf-proposed-success`: `test/src/specs/tx_pool/replace.rs::RbfReplaceProposedSuccess` (T1, T4, T6, T9, T10, T13) - A successful proposed replacement updates membership and template convergence once; committing its parent cannot wake retained victims while the winner still spends that output. Paired units: `authority::tests::chain::uak_chain_output_availability_respects_a_surviving_pool_spender`, `authority::tests::foundation::uak_rbf_replaces_the_complete_descendant_closure_atomically`, `authority::tests::template::uak_recovered_tree_has_normal_template_proposal_path`. Command: `make integration CKB_TEST_ARGS='-c 1 RbfReplaceProposedSuccess'`
- `rbf-proposed-template-refresh`: `test/src/specs/tx_pool/replace.rs::RbfRejectReplaceProposed` (T1, T4, T6, T9, T13) - Rejected proposed replacement is mutation-free and cannot publish a stale template generation. Paired units: `authority::tests::foundation::uak_rbf_replaces_the_complete_descendant_closure_atomically`, `authority::tests::template_driver::uak_template_driver_full_priority_and_partial_occ_use_one_output_revision`. Command: `make integration CKB_TEST_ARGS='-c 1 RbfRejectReplaceProposed'`

#### `TP-DEP-001` - Exact dependency and level-triggered progress

Generated focused command: `cargo nextest run -p ckb-tx-pool --features internal -E 'test(=authority::tests::dependency::uak_dependency_loss_is_exact_key_scoped) | test(=authority::tests::dependency::uak_direct_parent_acceptance_publishes_output_availability_atomically) | test(=authority::tests::dependency::uak_parent_terminalization_cannot_strand_trusted_child) | test(=authority::tests::dependency::uak_popular_dependency_maintenance_has_one_edge_steps_and_key_fairness) | test(=authority::tests::evidence_refinement::uak_dependency_evidence_currentness_refines_the_complete_finite_cut_relation) | test(=authority::tests::foundation::uak_coupled_membership_requires_exact_positive_input_evidence) | test(=authority::tests::refinement::uak_chain_and_pool_evidence_origins_refine_pointwise) | test(=authority::tests::refinement::uak_shared_headers_refine_as_commutative_reads) | test(=mathematical_model::adversarial_properties::model_shared_header_reads_are_explicit_and_commutative) | test(=mathematical_model::composition_properties::model_accepted_reader_relation_is_visible_before_ready_apply) | test(=mathematical_model::composition_properties::model_candidate_parent_child_relation_is_coupled) | test(=mathematical_model::composition_properties::model_pool_origin_routes_the_ready_head_to_the_coupled_planner) | test(=mathematical_model::composition_properties::model_reader_spender_relation_is_coupled_but_shared_readers_are_not) | test(=mathematical_model::composition_properties::model_shared_input_stops_at_the_first_coupled_ready_member) | test(=mathematical_model::composition_properties::model_shared_read_only_dependencies_and_headers_remain_composable) | test(=mathematical_model::dependency_progress_properties::model_dependency_completion_cannot_claim_owner_progress) | test(=mathematical_model::dependency_progress_properties::model_dependency_cursor_gives_key_fairness_without_a_second_queue) | test(=mathematical_model::dependency_progress_properties::model_dependency_event_only_supersedes_with_a_newer_cut) | test(=mathematical_model::dependency_progress_properties::model_dependency_event_without_an_affected_edge_creates_no_drain_work) | test(=mathematical_model::dependency_progress_properties::model_dependency_maintenance_consumes_one_finite_obligation_per_apply) | test(=mathematical_model::dependency_progress_properties::model_dependency_rank_bounds_requeue_pruning_under_a_stable_epoch) | test(=mathematical_model::dependency_progress_properties::model_dependency_rank_is_the_exact_static_edge_drain_bound) | test(=mathematical_model::develop_refinement::develop_edge_triggered_orphan_wake_can_lose_parent_progress) | test(=mathematical_model::evidence_transition_properties::model_dependency_proof_currentness_is_the_exact_loss_cut_order) | test(=mathematical_model::evidence_transition_properties::model_owner_free_resolution_and_missing_evidence_use_the_global_cut_exactly_once) | test(=mathematical_model::properties::model_capacity_apply_waits_remote_victim_dependents_and_stales_their_work) | test(=mathematical_model::properties::model_committed_parent_output_wakes_a_waiting_remote_child) | test(=mathematical_model::properties::model_definitive_parent_loss_closes_a_trusted_child_computing_verify) | test(=mathematical_model::properties::model_definitive_parent_loss_closes_a_trusted_child_queued_for_verify) | test(=mathematical_model::properties::model_definitive_parent_loss_waits_a_remote_child_and_stales_its_verify_work) | test(=mathematical_model::properties::model_definitive_preaccepted_parent_loss_cannot_strand_a_proposal_child) | test(=mathematical_model::properties::model_definitive_worker_failure_terminalizes_the_complete_dependency_closure) | test(=mathematical_model::properties::model_dependency_change_wakes_a_missing_child_in_the_same_apply) | test(=mathematical_model::properties::model_direct_membership_revalidates_a_chain_to_pool_origin_change) | test(=mathematical_model::properties::model_external_chain_availability_wakes_a_waiting_remote_child) | test(=mathematical_model::properties::model_header_availability_wakes_remote_without_requesting_a_parent_transaction) | test(=mathematical_model::properties::model_header_dependencies_are_chain_only_evidence_and_charged_edges) | test(=mathematical_model::properties::model_late_parent_is_coupled_and_rewrites_surviving_child_evidence_atomically) | test(=mathematical_model::properties::model_membership_rejection_closes_the_candidate_dependency_loss_in_one_apply) | test(=mathematical_model::properties::model_mixed_missing_dependencies_publish_only_cell_parent_requests_and_wake_partially) | test(=mathematical_model::properties::model_new_chain_loss_requeues_a_waiter_that_had_other_missing_evidence) | test(=mathematical_model::properties::model_pool_origin_is_evidence_and_parent_loss_removes_the_accepted_causal_closure) | test(=mathematical_model::properties::model_proposal_promotion_reclassifies_remote_wait_without_changing_owner_identity) | test(=mathematical_model::properties::model_ready_membership_revalidates_a_chain_to_pool_origin_change) | test(=mathematical_model::properties::model_refetchable_parent_removal_requeues_trusted_waiters_for_every_cause) | test(=mathematical_model::properties::model_trusted_missing_header_is_terminal_because_headers_are_chain_only) | test(=mathematical_model::properties::model_trusted_missing_policy_waits_only_for_a_preaccepted_cell_parent)'`

Rust evidence:

- `authority::tests::dependency::uak_dependency_loss_is_exact_key_scoped` (T4, T10)
- `authority::tests::dependency::uak_direct_parent_acceptance_publishes_output_availability_atomically` (T4, T6, T10)
- `authority::tests::dependency::uak_parent_terminalization_cannot_strand_trusted_child` (T4, T10)
- `authority::tests::dependency::uak_popular_dependency_maintenance_has_one_edge_steps_and_key_fairness` (T4, T8, T10)
- `authority::tests::evidence_refinement::uak_dependency_evidence_currentness_refines_the_complete_finite_cut_relation` (T2, T4, T6, T10, T11)
- `authority::tests::foundation::uak_coupled_membership_requires_exact_positive_input_evidence` (T4, T6, T11)
- `authority::tests::refinement::uak_chain_and_pool_evidence_origins_refine_pointwise` (T4, T6, T11)
- `authority::tests::refinement::uak_shared_headers_refine_as_commutative_reads` (T4, T6, T11)
- `mathematical_model::adversarial_properties::model_shared_header_reads_are_explicit_and_commutative` (T4, T6, T11)
- `mathematical_model::composition_properties::model_accepted_reader_relation_is_visible_before_ready_apply` (T4, T6, T11)
- `mathematical_model::composition_properties::model_candidate_parent_child_relation_is_coupled` (T4, T6)
- `mathematical_model::composition_properties::model_pool_origin_routes_the_ready_head_to_the_coupled_planner` (T4, T5, T6, T11)
- `mathematical_model::composition_properties::model_reader_spender_relation_is_coupled_but_shared_readers_are_not` (T4, T6, T11)
- `mathematical_model::composition_properties::model_shared_input_stops_at_the_first_coupled_ready_member` (T4, T6)
- `mathematical_model::composition_properties::model_shared_read_only_dependencies_and_headers_remain_composable` (T4, T5, T6, T11)
- `mathematical_model::dependency_progress_properties::model_dependency_completion_cannot_claim_owner_progress` (T4, T8, T10)
- `mathematical_model::dependency_progress_properties::model_dependency_cursor_gives_key_fairness_without_a_second_queue` (T4, T8, T10)
- `mathematical_model::dependency_progress_properties::model_dependency_event_only_supersedes_with_a_newer_cut` (T4, T8, T10)
- `mathematical_model::dependency_progress_properties::model_dependency_event_without_an_affected_edge_creates_no_drain_work` (T4, T8, T10)
- `mathematical_model::dependency_progress_properties::model_dependency_maintenance_consumes_one_finite_obligation_per_apply` (T4, T8, T10)
- `mathematical_model::dependency_progress_properties::model_dependency_rank_bounds_requeue_pruning_under_a_stable_epoch` (T4, T8, T10)
- `mathematical_model::dependency_progress_properties::model_dependency_rank_is_the_exact_static_edge_drain_bound` (T4, T8, T10)
- `mathematical_model::develop_refinement::develop_edge_triggered_orphan_wake_can_lose_parent_progress` (T4, T10)
- `mathematical_model::evidence_transition_properties::model_dependency_proof_currentness_is_the_exact_loss_cut_order` (T2, T4, T6, T10, T11)
- `mathematical_model::evidence_transition_properties::model_owner_free_resolution_and_missing_evidence_use_the_global_cut_exactly_once` (T2, T4, T6, T10, T11)
- `mathematical_model::properties::model_capacity_apply_waits_remote_victim_dependents_and_stales_their_work` (T1, T2, T3, T4, T6, T8, T10, T11)
- `mathematical_model::properties::model_committed_parent_output_wakes_a_waiting_remote_child` (T1, T4, T6, T9, T10)
- `mathematical_model::properties::model_definitive_parent_loss_closes_a_trusted_child_computing_verify` (T1, T2, T4, T6, T7, T10, T11)
- `mathematical_model::properties::model_definitive_parent_loss_closes_a_trusted_child_queued_for_verify` (T1, T4, T6, T7, T10, T11)
- `mathematical_model::properties::model_definitive_parent_loss_waits_a_remote_child_and_stales_its_verify_work` (T1, T2, T3, T4, T6, T8, T10, T11)
- `mathematical_model::properties::model_definitive_preaccepted_parent_loss_cannot_strand_a_proposal_child` (T1, T4, T6, T7, T10)
- `mathematical_model::properties::model_definitive_worker_failure_terminalizes_the_complete_dependency_closure` (T1, T2, T4, T6, T7, T10)
- `mathematical_model::properties::model_dependency_change_wakes_a_missing_child_in_the_same_apply` (T4, T10)
- `mathematical_model::properties::model_direct_membership_revalidates_a_chain_to_pool_origin_change` (T1, T2, T4, T6, T10, T11)
- `mathematical_model::properties::model_external_chain_availability_wakes_a_waiting_remote_child` (T4, T9, T10)
- `mathematical_model::properties::model_header_availability_wakes_remote_without_requesting_a_parent_transaction` (T3, T4, T7, T9, T10)
- `mathematical_model::properties::model_header_dependencies_are_chain_only_evidence_and_charged_edges` (T3, T4, T8, T9, T11)
- `mathematical_model::properties::model_late_parent_is_coupled_and_rewrites_surviving_child_evidence_atomically` (T1, T2, T4, T6, T10, T11)
- `mathematical_model::properties::model_membership_rejection_closes_the_candidate_dependency_loss_in_one_apply` (T1, T3, T4, T6, T7, T10)
- `mathematical_model::properties::model_mixed_missing_dependencies_publish_only_cell_parent_requests_and_wake_partially` (T3, T4, T7, T9, T10)
- `mathematical_model::properties::model_new_chain_loss_requeues_a_waiter_that_had_other_missing_evidence` (T1, T2, T4, T6, T9, T10)
- `mathematical_model::properties::model_pool_origin_is_evidence_and_parent_loss_removes_the_accepted_causal_closure` (T1, T4, T6, T10, T11)
- `mathematical_model::properties::model_proposal_promotion_reclassifies_remote_wait_without_changing_owner_identity` (T1, T2, T4, T6, T10, T11)
- `mathematical_model::properties::model_ready_membership_revalidates_a_chain_to_pool_origin_change` (T1, T2, T4, T6, T10, T11)
- `mathematical_model::properties::model_refetchable_parent_removal_requeues_trusted_waiters_for_every_cause` (T1, T2, T4, T6, T7, T10)
- `mathematical_model::properties::model_trusted_missing_header_is_terminal_because_headers_are_chain_only` (T1, T4, T6, T7, T10, T11)
- `mathematical_model::properties::model_trusted_missing_policy_waits_only_for_a_preaccepted_cell_parent` (T1, T4, T6, T7, T10, T11)

Process-level evidence:

- `accepted-descendants-after-parent-reorg`: `test/src/specs/tx_pool/pool_reconcile.rs::PoolResolveConflictAfterReorg` (T1, T3, T4, T6, T9, T10) - A detached accepted parent and surviving descendant closure reconcile as one valid owner generation. Paired units: `authority::tests::dependency::uak_parent_terminalization_cannot_strand_trusted_child`, `authority::tests::chain::uak_detached_parent_and_accepted_child_recover_parent_first`, `authority::tests::foundation::uak_capacity_eviction_removes_one_complete_causal_component`. Command: `make integration CKB_TEST_ARGS='-c 1 PoolResolveConflictAfterReorg'`
- `cell-dep-arrival-order`: `test/src/specs/tx_pool/dead_cell_deps.rs::CellBeingCellDepAndSpentInSameBlockTestGetBlockTemplate` (T4, T6, T11, T12, T13) - A selected cell-dep reader remains ordered before the spender regardless of arrival order. Paired units: `authority::tests::foundation::uak_coupled_membership_requires_exact_positive_input_evidence`, `authority::tests::template::uak_template_read_receipt_shares_order_and_complete_resolved_payload`. Command: `make integration CKB_TEST_ARGS='-c 1 CellBeingCellDepAndSpentInSameBlockTestGetBlockTemplate'`
- `conditional-reader-fanout`: `test/src/specs/tx_pool/limit.rs::TxPoolLimitAncestorCount` (T3, T4, T6, T8, T10, T13) - A high-fanout dependency shape remains bounded without corrupting accepted causal limits. Paired units: `authority::tests::dependency::uak_popular_dependency_maintenance_has_one_edge_steps_and_key_fairness`, `authority::tests::foundation::uak_capacity_eviction_removes_one_complete_causal_component`, `block_assembler::candidate_uncles::tests::full_container_keeps_a_hard_global_bound`. Command: `make integration CKB_TEST_ARGS='-c 1 TxPoolLimitAncestorCount'`
- `dependent-reorg-chain`: `test/src/specs/tx_pool/reorg_recovers_dependent.rs::ReorgRecoversDependentChain` (T1, T4, T6, T9, T10) - Detached dependent transactions recover parent-first without a lost dependency wake. Paired units: `authority::tests::dependency::uak_parent_terminalization_cannot_strand_trusted_child`, `authority::tests::chain::uak_detached_parent_and_accepted_child_recover_parent_first`. Command: `make integration CKB_TEST_ARGS='-c 1 ReorgRecoversDependentChain'`
- `dependent-reorg-transactions`: `test/src/specs/tx_pool/reorg_recovers_dependent.rs::ReorgRecoversDependentTxs` (T1, T4, T6, T9, T10) - Ordinary detached transaction recovery preserves dependency order and exact chain generation. Paired units: `authority::tests::dependency::uak_parent_terminalization_cannot_strand_trusted_child`, `authority::tests::chain::uak_detached_parent_and_accepted_child_recover_parent_first`. Command: `make integration CKB_TEST_ARGS='-c 1 ReorgRecoversDependentTxs'`
- `multi-parent-orphan-frontier`: `test/src/specs/tx_pool/orphan_tx.rs::TxPoolOrphanReverse` (T4, T6, T7, T10) - A complete multi-parent missing frontier and its relay request commit in one bounded transition. Paired units: `authority::tests::dependency::uak_direct_parent_acceptance_publishes_output_availability_atomically`, `authority::tests::effect::uak_remote_missing_wait_and_parent_request_share_one_backpressured_apply`. Command: `make integration CKB_TEST_ARGS='-c 1 TxPoolOrphanReverse'`
- `same-lane-relay-continuation`: `test/src/specs/tx_pool/txs_relay_order.rs::TxsRelayOrder` (T2, T4, T5, T8, T10) - Same-lane continuation preserves exact work-capability ownership, dependency order and bounded fair progress. Paired units: `authority::tests::dependency::uak_popular_dependency_maintenance_has_one_edge_steps_and_key_fairness`, `authority::tests::foundation::uak_resolve_to_verify_continuation_changes_no_authority_state`, `authority::tests::foundation::uak_independent_ready_order_is_invariant_to_worker_completion_permutations`. Command: `make integration CKB_TEST_ARGS='-c 1 TxsRelayOrder'`

#### `TP-CACHE-001` - Bounded replacement history and recovery

Generated focused command: `cargo nextest run -p ckb-tx-pool --features internal -E 'test(=authority::tests::chain::uak_chain_output_availability_respects_a_surviving_pool_spender) | test(=authority::tests::chain::uak_replacement_history_survives_winner_commit_and_wakes_after_reorg) | test(=authority::tests::foundation::uak_independent_rbf_churn_never_exceeds_replacement_history_budget) | test(=authority::tests::foundation::uak_replacement_history_requires_trusted_proposal_to_promote) | test(=authority::tests::foundation::uak_replacement_history_waits_for_every_observed_blocker) | test(=authority::tests::foundation::uak_replacement_history_wakes_only_on_newer_projected_availability) | test(=mathematical_model::properties::model_capacity_eviction_recovers_history_blocked_by_the_removed_winner) | test(=mathematical_model::properties::model_history_saturation_discards_the_complete_optional_set_without_losing_winner) | test(=mathematical_model::properties::model_nested_replacement_history_waits_for_every_observed_dependency) | test(=mathematical_model::properties::model_replacement_history_survives_commit_and_recovers_on_detach_availability) | test(=mathematical_model::properties::model_same_chain_spend_cannot_publish_false_availability_to_history)'`

Rust evidence:

- `authority::tests::chain::uak_chain_output_availability_respects_a_surviving_pool_spender` (T1, T4, T6, T9, T10)
- `authority::tests::chain::uak_replacement_history_survives_winner_commit_and_wakes_after_reorg` (T1, T4, T6, T9, T10)
- `authority::tests::foundation::uak_independent_rbf_churn_never_exceeds_replacement_history_budget` (T3, T8)
- `authority::tests::foundation::uak_replacement_history_requires_trusted_proposal_to_promote` (T1, T11)
- `authority::tests::foundation::uak_replacement_history_waits_for_every_observed_blocker` (T4, T10)
- `authority::tests::foundation::uak_replacement_history_wakes_only_on_newer_projected_availability` (T2, T4, T10)
- `mathematical_model::properties::model_capacity_eviction_recovers_history_blocked_by_the_removed_winner` (T1, T3, T4, T6, T8, T10)
- `mathematical_model::properties::model_history_saturation_discards_the_complete_optional_set_without_losing_winner` (T1, T3, T4, T6, T8)
- `mathematical_model::properties::model_nested_replacement_history_waits_for_every_observed_dependency` (T1, T3, T4, T6, T8, T10)
- `mathematical_model::properties::model_replacement_history_survives_commit_and_recovers_on_detach_availability` (T1, T3, T4, T6, T9, T10)
- `mathematical_model::properties::model_same_chain_spend_cannot_publish_false_availability_to_history` (T1, T3, T4, T6, T9, T10)

Process-level evidence:

- `failed-rbf-terminal`: `test/src/specs/tx_pool/replace.rs::RbfContainInvalidInput` (T1, T3, T6, T7, T8, T12) - A policy-rejected RBF candidate publishes per-hash rejection while preserving every accepted owner and creating no uncharged conflict-history residency. Paired units: `authority::tests::foundation::uak_terminal_outcome_and_effect_commit_together`, `authority::tests::foundation::uak_failed_rbf_fee_disposition_preserves_victims_and_terminalizes_candidate`, `authority::tests::foundation::uak_independent_rbf_churn_never_exceeds_replacement_history_budget`, `authority::tests::query::uak_owned_transaction_query_hides_replacement_history_and_reports_minimum_fee`. Command: `make integration CKB_TEST_ARGS='-c 1 RbfContainInvalidInput'`
- `rbf-basic`: `test/src/specs/tx_pool/replace.rs::RbfBasic` (T1, T3, T4, T6, T7, T10) - A valid replacement atomically installs the winner and bounded recovery disposition. Paired units: `authority::tests::foundation::uak_replacement_history_wakes_only_on_newer_projected_availability`, `authority::tests::foundation::uak_rbf_replaces_the_complete_descendant_closure_atomically`, `authority::tests::foundation::uak_failed_rbf_fee_disposition_preserves_victims_and_terminalizes_candidate`. Command: `make integration CKB_TEST_ARGS='-c 1 RbfBasic'`
- `rbf-concurrency`: `test/src/specs/tx_pool/replace.rs::RbfConcurrency` (T1, T2, T6, T10) - Concurrent replacements preserve one highest-fee winner; direct policy losers terminalize, while only contenders actually displaced from Accepted enter charged replacement history. Paired units: `authority::tests::foundation::uak_replacement_history_wakes_only_on_newer_projected_availability`, `authority::tests::foundation::uak_failed_rbf_fee_disposition_preserves_victims_and_terminalizes_candidate`. Command: `make integration CKB_TEST_ARGS='-c 1 RbfConcurrency'`
- `rbf-conflict-recovery`: `test/src/specs/tx_pool/orphan_tx_recovery.rs::RbfOrphanRecovery` (T1, T3, T4, T6, T7, T9, T10) - Replacement history recovers only after a newer exact blocker release and re-enters validation. Paired units: `authority::tests::chain::uak_replacement_history_survives_winner_commit_and_wakes_after_reorg`, `authority::tests::foundation::uak_failed_rbf_fee_disposition_preserves_victims_and_terminalizes_candidate`. Command: `make integration CKB_TEST_ARGS='-c 1 RbfOrphanRecovery'`
- `rbf-cycling-attack`: `test/src/specs/tx_pool/replace.rs::RbfCyclingAttack` (T1, T2, T3, T4, T6, T8, T10) - Replacement cycling stays mutation-safe, bounded and unable to self-wake retained history. Paired units: `authority::tests::foundation::uak_replacement_history_wakes_only_on_newer_projected_availability`, `authority::tests::foundation::uak_independent_rbf_churn_never_exceeds_replacement_history_budget`, `authority::tests::foundation::uak_rbf_replaces_the_complete_descendant_closure_atomically`. Command: `make integration CKB_TEST_ARGS='-c 1 RbfCyclingAttack'`
- `rbf-proposed-success`: `test/src/specs/tx_pool/replace.rs::RbfReplaceProposedSuccess` (T1, T4, T6, T9, T10, T13) - A successful proposed replacement updates membership and template convergence once; committing its parent cannot wake retained victims while the winner still spends that output. Paired units: `authority::tests::chain::uak_chain_output_availability_respects_a_surviving_pool_spender`, `authority::tests::foundation::uak_rbf_replaces_the_complete_descendant_closure_atomically`, `authority::tests::template::uak_recovered_tree_has_normal_template_proposal_path`. Command: `make integration CKB_TEST_ARGS='-c 1 RbfReplaceProposedSuccess'`

#### `TP-BUDGET-001` - Continuous hostile-resource accounting

Generated focused command: `cargo nextest run -p ckb-tx-pool --features internal -E 'test(=authority::tests::dependency::uak_missing_growth_is_charged_or_becomes_budget_denied) | test(=authority::tests::foundation::uak_admission_must_fit_the_static_compute_envelope) | test(=authority::tests::foundation::uak_compute_grant_and_settlement_share_total_retained_byte_units) | test(=authority::tests::foundation::uak_full_retained_budget_cannot_hide_the_trusted_owner) | test(=authority::tests::foundation::uak_resource_limit_failure_preserves_every_observable_fact) | test(=authority::tests::foundation::uak_resource_reference_rejects_ghost_overcharge) | test(=authority::tests::ingress_batch::uak_retained_ingress_batch_applies_resource_pressure_sequentially) | test(=authority::tests::refinement::uak_accepted_entry_capacity_refines_every_ready_prefix) | test(=authority::tests::resource_refinement::uak_compute_release_refines_exact_reservation_and_peer_attribution_exhaustively) | test(=authority::tests::resource_refinement::uak_resource_charge_and_batch_refine_the_finite_set_transition_exhaustively) | test(=authority::tests::resource_refinement::uak_resource_vectors_and_configuration_refine_the_finite_algebra_exhaustively) | test(=mathematical_model::composition_properties::model_ready_prefix_stops_before_aggregate_accepted_capacity_exclusion) | test(=mathematical_model::develop_refinement::develop_fragmented_limits_can_pass_without_one_total_retained_budget) | test(=mathematical_model::properties::model_capacity_eviction_never_removes_a_candidate_ancestor) | test(=mathematical_model::properties::model_capacity_eviction_removes_one_complete_accepted_component) | test(=mathematical_model::properties::model_capacity_eviction_uses_status_weight_count_and_arrival_in_order) | test(=mathematical_model::properties::model_capacity_self_eviction_preserves_existing_membership_and_terminalizes_the_candidate) | test(=mathematical_model::properties::model_compute_grant_uses_total_retained_bytes_not_payload_only_evidence) | test(=mathematical_model::properties::model_per_peer_resource_exclusion_is_pre_apply_and_other_peers_remain_independent) | test(=mathematical_model::properties::model_startup_rejects_recovery_history_larger_than_retained_capacity) | test(=mathematical_model::resource_transition_properties::model_charge_validation_exhausts_partial_compute_reservations) | test(=mathematical_model::resource_transition_properties::model_compute_release_consumes_the_exact_reservation_and_peer_attribution) | test(=mathematical_model::resource_transition_properties::model_compute_reservation_is_all_or_nothing_and_preserves_edge_units) | test(=mathematical_model::resource_transition_properties::model_resource_batch_is_one_order_independent_set_transition) | test(=mathematical_model::resource_transition_properties::model_resource_configuration_rejects_each_independent_invalid_premise)'`

Rust evidence:

- `authority::tests::dependency::uak_missing_growth_is_charged_or_becomes_budget_denied` (T3, T4, T8)
- `authority::tests::foundation::uak_admission_must_fit_the_static_compute_envelope` (T3, T8)
- `authority::tests::foundation::uak_compute_grant_and_settlement_share_total_retained_byte_units` (T3, T6, T8, T11)
- `authority::tests::foundation::uak_full_retained_budget_cannot_hide_the_trusted_owner` (T3, T8)
- `authority::tests::foundation::uak_resource_limit_failure_preserves_every_observable_fact` (T3, T6, T8)
- `authority::tests::foundation::uak_resource_reference_rejects_ghost_overcharge` (T1, T3)
- `authority::tests::ingress_batch::uak_retained_ingress_batch_applies_resource_pressure_sequentially` (T1, T3, T6, T8)
- `authority::tests::refinement::uak_accepted_entry_capacity_refines_every_ready_prefix` (T3, T6, T8)
- `authority::tests::resource_refinement::uak_compute_release_refines_exact_reservation_and_peer_attribution_exhaustively` (T1, T3, T6, T8)
- `authority::tests::resource_refinement::uak_resource_charge_and_batch_refine_the_finite_set_transition_exhaustively` (T1, T3, T6, T8)
- `authority::tests::resource_refinement::uak_resource_vectors_and_configuration_refine_the_finite_algebra_exhaustively` (T1, T3, T6, T8)
- `mathematical_model::composition_properties::model_ready_prefix_stops_before_aggregate_accepted_capacity_exclusion` (T3, T6, T8)
- `mathematical_model::develop_refinement::develop_fragmented_limits_can_pass_without_one_total_retained_budget` (T3, T8)
- `mathematical_model::properties::model_capacity_eviction_never_removes_a_candidate_ancestor` (T1, T3, T4, T6, T8)
- `mathematical_model::properties::model_capacity_eviction_removes_one_complete_accepted_component` (T1, T3, T4, T6, T7, T8)
- `mathematical_model::properties::model_capacity_eviction_uses_status_weight_count_and_arrival_in_order` (T1, T3, T5, T6, T8, T11)
- `mathematical_model::properties::model_capacity_self_eviction_preserves_existing_membership_and_terminalizes_the_candidate` (T1, T3, T6, T7, T8)
- `mathematical_model::properties::model_compute_grant_uses_total_retained_bytes_not_payload_only_evidence` (T3, T8, T11)
- `mathematical_model::properties::model_per_peer_resource_exclusion_is_pre_apply_and_other_peers_remain_independent` (T3, T6, T8)
- `mathematical_model::properties::model_startup_rejects_recovery_history_larger_than_retained_capacity` (T3, T8)
- `mathematical_model::resource_transition_properties::model_charge_validation_exhausts_partial_compute_reservations` (T1, T3, T6, T8)
- `mathematical_model::resource_transition_properties::model_compute_release_consumes_the_exact_reservation_and_peer_attribution` (T1, T3, T6, T8)
- `mathematical_model::resource_transition_properties::model_compute_reservation_is_all_or_nothing_and_preserves_edge_units` (T1, T3, T6, T8)
- `mathematical_model::resource_transition_properties::model_resource_batch_is_one_order_independent_set_transition` (T1, T3, T6, T8)
- `mathematical_model::resource_transition_properties::model_resource_configuration_rejects_each_independent_invalid_premise` (T1, T3, T6, T8)

Cross-crate Rust evidence:

Generated cross-crate command: `cargo nextest run -p ckb-app-config -E 'test(=legacy::tx_pool::tests::explicit_unified_pipeline_budget_takes_precedence_over_legacy_input) | test(=legacy::tx_pool::tests::legacy_verify_budget_preserves_the_old_aggregate_pipeline_capacity) | test(=legacy::tx_pool::tests::omitted_pipeline_budget_keeps_the_unified_default)'`
Generated cross-crate command: `cargo nextest run -p ckb-sync -E 'test(=relayer::tests::block_proposal_process::accepted_proposal_consumes_inflight_and_marks_known) | test(=relayer::tests::block_proposal_process::test_clear_expired_inflight_proposals) | test(=relayer::tests::block_proposal_process::test_no_asked) | test(=relayer::tests::block_proposal_process::test_no_unknown) | test(=relayer::tests::block_proposal_process::test_oversized_batch_is_rejected_before_relay_state_changes)'`

- `util/app-config/src/legacy/tx_pool.rs::legacy::tx_pool::tests::explicit_unified_pipeline_budget_takes_precedence_over_legacy_input` (T3, T8) - An explicit current pipeline budget has deterministic precedence over its legacy translation input.
- `util/app-config/src/legacy/tx_pool.rs::legacy::tx_pool::tests::legacy_verify_budget_preserves_the_old_aggregate_pipeline_capacity` (T3, T8) - A legacy verify-queue budget is translated to the unified pipeline budget without shrinking the old aggregate retained capacity.
- `util/app-config/src/legacy/tx_pool.rs::legacy::tx_pool::tests::omitted_pipeline_budget_keeps_the_unified_default` (T3, T8) - A legacy configuration omitting every new tx-pool field receives the validated resident, pipeline and arrival-order compatibility defaults.
- `sync/src/relayer/tests/block_proposal_process.rs::relayer::tests::block_proposal_process::accepted_proposal_consumes_inflight_and_marks_known` (T1, T6, T7, T8, T10) - A requested valid Proposal consumes its network request when the response arrives, remains unknown during authority validation, and becomes known only through the committed Accepted effect.
- `sync/src/relayer/tests/block_proposal_process.rs::relayer::tests::block_proposal_process::test_clear_expired_inflight_proposals` (T7, T8, T10) - Expired proposal requests are cleared without retaining stale relay state.
- `sync/src/relayer/tests/block_proposal_process.rs::relayer::tests::block_proposal_process::test_no_asked` (T7, T8) - An unsolicited valid proposal transaction does not enter the known projection.
- `sync/src/relayer/tests/block_proposal_process.rs::relayer::tests::block_proposal_process::test_no_unknown` (T7, T8) - A known proposal transaction is ignored without creating duplicate relay work.
- `sync/src/relayer/tests/block_proposal_process.rs::relayer::tests::block_proposal_process::test_oversized_batch_is_rejected_before_relay_state_changes` (T6, T8, T11) - RelayV3 byte bounds reject an oversized proposal batch before inflight or known state changes.

#### `TP-WORKER-001` - Capability-owned workers and progress

Generated focused command: `cargo nextest run -p ckb-tx-pool --features internal -E 'test(=authority::runtime::tests::runtime_checkout_observes_preexisting_level_without_a_wake_hint) | test(=authority::runtime::tests::runtime_compute_coordinator_drains_a_coalesced_preexisting_frontier) | test(=authority::runtime::tests::runtime_compute_wake_coalesces_role_heads_without_becoming_authority) | test(=authority::runtime::tests::runtime_single_any_verifier_settles_mixed_preexisting_frontier) | test(=authority::tests::boundary_refinement::uak_topology_lifecycle_refines_running_and_stopped_cuts) | test(=authority::tests::compute_coordinator::uak_closed_assignment_transport_returns_the_exact_checked_out_capability) | test(=authority::tests::compute_coordinator::uak_compute_coordinator_probes_every_role_with_one_available_fair_permit) | test(=authority::tests::compute_coordinator::uak_effect_blocked_completion_observes_a_later_fair_permit_release) | test(=authority::tests::compute_exchange::uak_compute_exchange_refines_the_named_no_interleave_settle_refill_fold) | test(=authority::tests::compute_exchange::uak_compute_exchange_rejects_a_capability_partition_larger_than_the_worker_topology) | test(=authority::tests::compute_exchange::uak_compute_exchange_uses_the_configured_active_work_bound) | test(=authority::tests::compute_exchange::uak_initial_compute_exchange_checks_out_one_available_worker_wave_with_one_stamp) | test(=authority::tests::foundation::uak_fair_frontier_is_a_derived_non_owning_projection) | test(=authority::tests::foundation::uak_fair_frontier_round_robins_owners_only_after_apply) | test(=authority::tests::foundation::uak_resolve_to_verify_continuation_changes_no_authority_state) | test(=authority::tests::foundation::uak_runner_cancellation_settles_one_exact_work_capability_before_exit) | test(=authority::tests::progress_refinement::uak_wake_transition_refines_every_finite_projection_law) | test(=authority::tests::scheduler_refinement::uak_multi_owner_resolve_wave_refines_the_scheduler_quotient_pointwise) | test(=authority::tests::scheduler_refinement::uak_verify_capability_wave_refines_the_scheduler_quotient_pointwise) | test(=authority::tests::scheduler_refinement::uak_verify_order_modes_refine_the_scheduler_quotient_pointwise) | test(=authority::tests::scheduler_transition_refinement::uak_scheduler_owner_ring_refines_every_finite_union_cursor_and_blocked_set) | test(=authority::tests::scheduler_transition_refinement::uak_scheduler_private_partial_orders_equal_total_cmp_for_real_keys) | test(=authority::tests::scheduler_transition_refinement::uak_scheduler_set_transition_refines_every_real_projection_state_and_order) | test(=authority::tests::trace_refinement::uak_authority_lifecycle_traces_refine_model_at_every_stable_cut) | test(=authority::tests::worker::uak_idle_maintenance_driver_waits_instead_of_spinning) | test(=authority::tests::worker::uak_maintenance_driver_fairly_drains_every_preexisting_level) | test(=benchmark::debug_tests::controller_repeated_idle_then_burst_never_loses_compute_wake) | test(=mathematical_model::adversarial_properties::model_completion_plan_is_equal_for_every_bounded_worker_permutation) | test(=mathematical_model::adversarial_properties::model_same_cut_revalidation_is_no_progress_but_new_revision_is_new_evidence) | test(=mathematical_model::composition_properties::model_completion_drain_is_canonical_across_arrival_order) | test(=mathematical_model::composition_properties::model_completion_drain_never_waits_for_a_slow_batch_peer) | test(=mathematical_model::composition_properties::model_completion_drain_rejections_return_every_linear_token) | test(=mathematical_model::composition_properties::model_compute_exchange_is_invariant_to_worker_completion_order) | test(=mathematical_model::composition_properties::model_compute_exchange_settles_and_refills_all_available_slots_in_one_apply) | test(=mathematical_model::composition_properties::model_duplicate_permit_identity_is_rejected_without_token_loss) | test(=mathematical_model::composition_properties::model_finished_acquirer_never_queues_behind_a_direct_waiter) | test(=mathematical_model::composition_properties::model_finished_result_holds_its_worker_slot_until_the_exchange_settles_it) | test(=mathematical_model::composition_properties::model_foreign_release_and_mixed_domain_batch_rejection_return_exact_tokens) | test(=mathematical_model::composition_properties::model_foreign_scheduler_and_plan_rejections_return_exact_linear_tokens) | test(=mathematical_model::composition_properties::model_initial_compute_wave_checks_out_every_available_worker_in_one_apply) | test(=mathematical_model::composition_properties::model_local_waiter_receives_the_released_permit_before_retained_reuse) | test(=mathematical_model::composition_properties::model_retained_acquirer_queues_once_then_only_fills_immediately_available_slots) | test(=mathematical_model::composition_properties::model_stale_completion_drain_returns_the_exact_execution_capability) | test(=mathematical_model::composition_properties::model_stale_compute_exchange_returns_every_fair_grant_without_mutation) | test(=mathematical_model::progress_properties::model_compute_wake_is_exact_over_the_finite_head_and_release_algebra) | test(=mathematical_model::progress_properties::model_effect_capacity_wake_observes_any_release_but_not_stable_or_growth) | test(=mathematical_model::progress_properties::model_wake_relation_separates_suppression_from_spurious_cost) | test(=mathematical_model::properties::model_cancel_retires_a_checked_out_capability_exactly_once) | test(=mathematical_model::properties::model_compute_completion_returns_to_the_fair_arbiter_before_new_work_can_reuse_it) | test(=mathematical_model::properties::model_continuous_resolve_falls_back_when_worker_cannot_verify_the_resolved_class) | test(=mathematical_model::properties::model_continuous_resolve_verify_preserves_one_capability_and_exact_apply_cost) | test(=mathematical_model::properties::model_direct_negative_receipt_ignores_unrelated_commits_but_detects_relevant_change) | test(=mathematical_model::properties::model_direct_positive_dependency_loss_is_a_relevant_change_not_policy_rejection) | test(=mathematical_model::properties::model_post_apply_assignment_and_completion_delivery_failure_return_the_exact_capability) | test(=mathematical_model::properties::model_ready_capture_never_skips_a_new_stronger_head) | test(=mathematical_model::properties::model_resolve_continuation_rejects_incompatible_or_stale_evidence_without_mutation) | test(=mathematical_model::properties::model_verification_suspend_blocks_checkout_without_discarding_active_work) | test(=mathematical_model::scheduler_properties::model_compute_exchange_rejects_a_stale_scheduler_cut_and_returns_its_grant) | test(=mathematical_model::scheduler_properties::model_scheduler_cursor_persists_across_committed_compute_waves) | test(=mathematical_model::scheduler_properties::model_scheduler_wave_binds_canonical_worker_roles_to_distinct_fair_owners) | test(=mathematical_model::scheduler_properties::model_scheduler_wave_preserves_verify_capability_and_configured_order) | test(=mathematical_model::scheduler_properties::model_worker_grant_binding_rejects_ambiguous_topology_without_losing_resources) | test(=mathematical_model::scheduler_transition_properties::model_scheduler_batch_is_one_order_independent_set_transition) | test(=mathematical_model::scheduler_transition_properties::model_scheduler_duplicate_overlay_owner_is_a_bounded_probe_not_a_second_owner) | test(=mathematical_model::scheduler_transition_properties::model_scheduler_overlay_eligibility_is_union_not_intersection_or_constant) | test(=mathematical_model::scheduler_transition_properties::model_scheduler_owner_bound_and_next_after_skip_each_ineligible_owner_once) | test(=mathematical_model::scheduler_transition_properties::model_scheduler_partial_order_is_exactly_some_total_cmp) | test(=mathematical_model::scheduler_transition_properties::model_scheduler_ready_is_in_the_set_projection_and_outside_the_worker_ring) | test(=mathematical_model::scheduler_transition_properties::model_scheduler_replace_rejects_stale_duplicate_and_identity_ambiguous_changes) | test(=mathematical_model::topology_properties::model_finished_exchange_never_waits_for_a_fair_permit_before_settlement) | test(=mathematical_model::trace::tests::reference_lifecycle_domain_preserves_every_stable_cut) | test(=mathematical_model::trace::tests::trace_lifecycle_domain_is_the_complete_class_route_product)'`

Rust evidence:

- `authority::runtime::tests::runtime_checkout_observes_preexisting_level_without_a_wake_hint` (T2, T10)
- `authority::runtime::tests::runtime_compute_coordinator_drains_a_coalesced_preexisting_frontier` (T2, T5, T10, T15, T16)
- `authority::runtime::tests::runtime_compute_wake_coalesces_role_heads_without_becoming_authority` (T2, T5, T10)
- `authority::runtime::tests::runtime_single_any_verifier_settles_mixed_preexisting_frontier` (T2, T5, T10, T15)
- `authority::tests::boundary_refinement::uak_topology_lifecycle_refines_running_and_stopped_cuts` (T2, T7, T10)
- `authority::tests::compute_coordinator::uak_closed_assignment_transport_returns_the_exact_checked_out_capability` (T2, T6, T10, T15)
- `authority::tests::compute_coordinator::uak_compute_coordinator_probes_every_role_with_one_available_fair_permit` (T2, T5, T8, T10, T15, T16)
- `authority::tests::compute_coordinator::uak_effect_blocked_completion_observes_a_later_fair_permit_release` (T2, T3, T5, T7, T8, T10, T15)
- `authority::tests::compute_exchange::uak_compute_exchange_refines_the_named_no_interleave_settle_refill_fold` (T2, T5, T6, T8, T10, T14, T15, T16)
- `authority::tests::compute_exchange::uak_compute_exchange_rejects_a_capability_partition_larger_than_the_worker_topology` (T2, T3, T6, T8, T15)
- `authority::tests::compute_exchange::uak_compute_exchange_uses_the_configured_active_work_bound` (T3, T6, T8, T15)
- `authority::tests::compute_exchange::uak_initial_compute_exchange_checks_out_one_available_worker_wave_with_one_stamp` (T2, T5, T6, T10, T14, T15, T16)
- `authority::tests::foundation::uak_fair_frontier_is_a_derived_non_owning_projection` (T2, T5, T6, T10)
- `authority::tests::foundation::uak_fair_frontier_round_robins_owners_only_after_apply` (T5, T10)
- `authority::tests::foundation::uak_resolve_to_verify_continuation_changes_no_authority_state` (T2, T5, T10)
- `authority::tests::foundation::uak_runner_cancellation_settles_one_exact_work_capability_before_exit` (T2, T10)
- `authority::tests::progress_refinement::uak_wake_transition_refines_every_finite_projection_law` (T4, T5, T7, T8, T10, T13)
- `authority::tests::scheduler_refinement::uak_multi_owner_resolve_wave_refines_the_scheduler_quotient_pointwise` (T2, T5, T6, T8, T10)
- `authority::tests::scheduler_refinement::uak_verify_capability_wave_refines_the_scheduler_quotient_pointwise` (T2, T5, T6, T8, T10)
- `authority::tests::scheduler_refinement::uak_verify_order_modes_refine_the_scheduler_quotient_pointwise` (T2, T5, T6, T8, T10)
- `authority::tests::scheduler_transition_refinement::uak_scheduler_owner_ring_refines_every_finite_union_cursor_and_blocked_set` (T2, T5, T6, T8, T10)
- `authority::tests::scheduler_transition_refinement::uak_scheduler_private_partial_orders_equal_total_cmp_for_real_keys` (T2, T5, T6, T8, T10)
- `authority::tests::scheduler_transition_refinement::uak_scheduler_set_transition_refines_every_real_projection_state_and_order` (T2, T5, T6, T8, T10)
- `authority::tests::trace_refinement::uak_authority_lifecycle_traces_refine_model_at_every_stable_cut` (T1, T2, T3, T5, T6, T8, T10, T11, T12)
- `authority::tests::worker::uak_idle_maintenance_driver_waits_instead_of_spinning` (T10)
- `authority::tests::worker::uak_maintenance_driver_fairly_drains_every_preexisting_level` (T4, T8, T10)
- `benchmark::debug_tests::controller_repeated_idle_then_burst_never_loses_compute_wake` (T2, T5, T10)
- `mathematical_model::adversarial_properties::model_completion_plan_is_equal_for_every_bounded_worker_permutation` (T2, T5, T6, T10)
- `mathematical_model::adversarial_properties::model_same_cut_revalidation_is_no_progress_but_new_revision_is_new_evidence` (T2, T9, T10, T11)
- `mathematical_model::composition_properties::model_completion_drain_is_canonical_across_arrival_order` (T2, T5, T6, T10, T14, T15)
- `mathematical_model::composition_properties::model_completion_drain_never_waits_for_a_slow_batch_peer` (T2, T5, T10, T15, T16)
- `mathematical_model::composition_properties::model_completion_drain_rejections_return_every_linear_token` (T2, T6, T10, T15)
- `mathematical_model::composition_properties::model_compute_exchange_is_invariant_to_worker_completion_order` (T2, T5, T6, T10, T14, T15)
- `mathematical_model::composition_properties::model_compute_exchange_settles_and_refills_all_available_slots_in_one_apply` (T2, T5, T6, T10, T14, T15, T16)
- `mathematical_model::composition_properties::model_duplicate_permit_identity_is_rejected_without_token_loss` (T2, T5, T6, T8)
- `mathematical_model::composition_properties::model_finished_acquirer_never_queues_behind_a_direct_waiter` (T2, T5, T8, T10)
- `mathematical_model::composition_properties::model_finished_result_holds_its_worker_slot_until_the_exchange_settles_it` (T2, T3, T5, T8, T10, T15)
- `mathematical_model::composition_properties::model_foreign_release_and_mixed_domain_batch_rejection_return_exact_tokens` (T2, T5, T6, T8)
- `mathematical_model::composition_properties::model_foreign_scheduler_and_plan_rejections_return_exact_linear_tokens` (T2, T5, T6, T8)
- `mathematical_model::composition_properties::model_initial_compute_wave_checks_out_every_available_worker_in_one_apply` (T2, T5, T6, T10, T15, T16)
- `mathematical_model::composition_properties::model_local_waiter_receives_the_released_permit_before_retained_reuse` (T2, T5, T10)
- `mathematical_model::composition_properties::model_retained_acquirer_queues_once_then_only_fills_immediately_available_slots` (T2, T5, T8, T10)
- `mathematical_model::composition_properties::model_stale_completion_drain_returns_the_exact_execution_capability` (T2, T6, T10)
- `mathematical_model::composition_properties::model_stale_compute_exchange_returns_every_fair_grant_without_mutation` (T2, T5, T6, T10, T15)
- `mathematical_model::progress_properties::model_compute_wake_is_exact_over_the_finite_head_and_release_algebra` (T4, T5, T7, T8, T10, T13)
- `mathematical_model::progress_properties::model_effect_capacity_wake_observes_any_release_but_not_stable_or_growth` (T4, T5, T7, T8, T10, T13)
- `mathematical_model::progress_properties::model_wake_relation_separates_suppression_from_spurious_cost` (T4, T5, T7, T8, T10, T13)
- `mathematical_model::properties::model_cancel_retires_a_checked_out_capability_exactly_once` (T2, T10)
- `mathematical_model::properties::model_compute_completion_returns_to_the_fair_arbiter_before_new_work_can_reuse_it` (T2, T5, T10)
- `mathematical_model::properties::model_continuous_resolve_falls_back_when_worker_cannot_verify_the_resolved_class` (T2, T3, T5, T6, T8, T10, T11)
- `mathematical_model::properties::model_continuous_resolve_verify_preserves_one_capability_and_exact_apply_cost` (T2, T3, T5, T6, T8, T10, T11, T12)
- `mathematical_model::properties::model_direct_negative_receipt_ignores_unrelated_commits_but_detects_relevant_change` (T2, T10, T11)
- `mathematical_model::properties::model_direct_positive_dependency_loss_is_a_relevant_change_not_policy_rejection` (T2, T4, T10, T11)
- `mathematical_model::properties::model_post_apply_assignment_and_completion_delivery_failure_return_the_exact_capability` (T2, T6, T10)
- `mathematical_model::properties::model_ready_capture_never_skips_a_new_stronger_head` (T2, T5, T10)
- `mathematical_model::properties::model_resolve_continuation_rejects_incompatible_or_stale_evidence_without_mutation` (T2, T6, T11)
- `mathematical_model::properties::model_verification_suspend_blocks_checkout_without_discarding_active_work` (T2, T5, T10)
- `mathematical_model::scheduler_properties::model_compute_exchange_rejects_a_stale_scheduler_cut_and_returns_its_grant` (T2, T5, T6, T8, T10, T15)
- `mathematical_model::scheduler_properties::model_scheduler_cursor_persists_across_committed_compute_waves` (T2, T5, T6, T10)
- `mathematical_model::scheduler_properties::model_scheduler_wave_binds_canonical_worker_roles_to_distinct_fair_owners` (T2, T5, T6, T8, T10)
- `mathematical_model::scheduler_properties::model_scheduler_wave_preserves_verify_capability_and_configured_order` (T2, T5, T6, T8, T10)
- `mathematical_model::scheduler_properties::model_worker_grant_binding_rejects_ambiguous_topology_without_losing_resources` (T2, T5, T6, T8)
- `mathematical_model::scheduler_transition_properties::model_scheduler_batch_is_one_order_independent_set_transition` (T2, T5, T6, T8, T10)
- `mathematical_model::scheduler_transition_properties::model_scheduler_duplicate_overlay_owner_is_a_bounded_probe_not_a_second_owner` (T2, T5, T6, T8, T10)
- `mathematical_model::scheduler_transition_properties::model_scheduler_overlay_eligibility_is_union_not_intersection_or_constant` (T2, T5, T6, T8, T10)
- `mathematical_model::scheduler_transition_properties::model_scheduler_owner_bound_and_next_after_skip_each_ineligible_owner_once` (T2, T5, T6, T8, T10)
- `mathematical_model::scheduler_transition_properties::model_scheduler_partial_order_is_exactly_some_total_cmp` (T2, T5, T6, T8, T10)
- `mathematical_model::scheduler_transition_properties::model_scheduler_ready_is_in_the_set_projection_and_outside_the_worker_ring` (T2, T5, T6, T8, T10)
- `mathematical_model::scheduler_transition_properties::model_scheduler_replace_rejects_stale_duplicate_and_identity_ambiguous_changes` (T2, T5, T6, T8, T10)
- `mathematical_model::topology_properties::model_finished_exchange_never_waits_for_a_fair_permit_before_settlement` (T2, T5, T8, T10, T15, T16)
- `mathematical_model::trace::tests::reference_lifecycle_domain_preserves_every_stable_cut` (T1, T2, T3, T5, T6, T8, T10, T11)
- `mathematical_model::trace::tests::trace_lifecycle_domain_is_the_complete_class_route_product` (T2, T5, T8, T11, T12)

Process-level evidence:

- `same-lane-relay-continuation`: `test/src/specs/tx_pool/txs_relay_order.rs::TxsRelayOrder` (T2, T4, T5, T8, T10) - Same-lane continuation preserves exact work-capability ownership, dependency order and bounded fair progress. Paired units: `authority::tests::dependency::uak_popular_dependency_maintenance_has_one_edge_steps_and_key_fairness`, `authority::tests::foundation::uak_resolve_to_verify_continuation_changes_no_authority_state`, `authority::tests::foundation::uak_independent_ready_order_is_invariant_to_worker_completion_permutations`. Command: `make integration CKB_TEST_ARGS='-c 1 TxsRelayOrder'`

#### `TP-ADMIN-001` - Cause-complete administration and peer revocation

Generated focused command: `cargo nextest run -p ckb-tx-pool --features internal -E 'test(=authority::tests::ban::saturation_evicts_the_oldest_fence_and_keeps_a_hard_bound) | test(=authority::tests::compute_exchange::uak_blocked_multi_peer_revocations_exclude_every_deferred_peer_from_refill) | test(=authority::tests::compute_exchange::uak_malformed_verify_completion_revokes_its_remote_peer_cohort) | test(=authority::tests::compute_exchange::uak_nonmalformed_verify_completion_never_revokes_its_remote_peer) | test(=authority::tests::foundation::uak_accepted_expiry_uses_stable_deadlines_and_expires_the_full_closure) | test(=authority::tests::foundation::uak_clear_pipeline_preserves_accepted_and_invalidates_active_work) | test(=authority::tests::foundation::uak_local_non_remote_preaccepted_removal_does_not_release_relay_state) | test(=authority::tests::foundation::uak_peer_revocation_removes_active_owner_and_stales_checked_out_work) | test(=authority::tests::foundation::uak_peer_revocation_removes_only_preaccepted_ingress_owners) | test(=authority::tests::foundation::uak_remote_expiry_is_a_bounded_derived_transition_and_allows_refetch) | test(=authority::tests::foundation::uak_runtime_clear_scopes_and_snapshot_pairing_are_indivisible) | test(=authority::tests::ingress::uak_delayed_revoked_remote_ingress_commits_a_later_filter_release) | test(=authority::tests::ingress::uak_peer_fence_saturation_revalidates_the_oldest_delayed_session) | test(=authority::tests::ingress_batch::uak_malformed_remote_batch_revokes_the_peer_before_any_batch_owner_survives) | test(=mathematical_model::adversarial_properties::model_disjoint_peer_work_is_unchanged_by_an_unrelated_peer_ban) | test(=mathematical_model::adversarial_properties::model_hostile_trace_makes_wall_and_monotonic_clock_domains_explicit) | test(=mathematical_model::develop_refinement::develop_peer_ban_can_miss_checked_out_remote_work) | test(=mathematical_model::properties::model_peer_ban_fence_is_bounded_and_expires_in_the_monotonic_clock_domain) | test(=mathematical_model::properties::model_peer_ban_releases_only_matching_pre_authority_relay_handoffs) | test(=mathematical_model::properties::model_peer_ban_removes_only_retained_remote_owners_and_releases_refetch) | test(=mathematical_model::properties::model_remote_expiry_is_bounded_canonical_and_ignores_promoted_residency) | test(=mathematical_model::properties::model_wall_clock_rollback_never_fabricates_expiry_progress)'`

Rust evidence:

- `authority::tests::ban::saturation_evicts_the_oldest_fence_and_keeps_a_hard_bound` (T8, T10)
- `authority::tests::compute_exchange::uak_blocked_multi_peer_revocations_exclude_every_deferred_peer_from_refill` (T1, T2, T3, T6, T7, T8, T10, T11)
- `authority::tests::compute_exchange::uak_malformed_verify_completion_revokes_its_remote_peer_cohort` (T1, T2, T3, T6, T7, T11)
- `authority::tests::compute_exchange::uak_nonmalformed_verify_completion_never_revokes_its_remote_peer` (T1, T2, T6, T7, T11)
- `authority::tests::foundation::uak_accepted_expiry_uses_stable_deadlines_and_expires_the_full_closure` (T3, T4, T6)
- `authority::tests::foundation::uak_clear_pipeline_preserves_accepted_and_invalidates_active_work` (T1, T2, T3, T6)
- `authority::tests::foundation::uak_local_non_remote_preaccepted_removal_does_not_release_relay_state` (T7, T11)
- `authority::tests::foundation::uak_peer_revocation_removes_active_owner_and_stales_checked_out_work` (T1, T2, T3, T6, T10)
- `authority::tests::foundation::uak_peer_revocation_removes_only_preaccepted_ingress_owners` (T1, T3, T7)
- `authority::tests::foundation::uak_remote_expiry_is_a_bounded_derived_transition_and_allows_refetch` (T1, T3, T7, T10)
- `authority::tests::foundation::uak_runtime_clear_scopes_and_snapshot_pairing_are_indivisible` (T1, T2, T3, T6, T9, T12)
- `authority::tests::ingress::uak_delayed_revoked_remote_ingress_commits_a_later_filter_release` (T1, T6, T7, T10)
- `authority::tests::ingress::uak_peer_fence_saturation_revalidates_the_oldest_delayed_session` (T3, T8, T10, T11)
- `authority::tests::ingress_batch::uak_malformed_remote_batch_revokes_the_peer_before_any_batch_owner_survives` (T1, T3, T6, T7)
- `mathematical_model::adversarial_properties::model_disjoint_peer_work_is_unchanged_by_an_unrelated_peer_ban` (T1, T3, T6, T8, T10)
- `mathematical_model::adversarial_properties::model_hostile_trace_makes_wall_and_monotonic_clock_domains_explicit` (T3, T8, T10)
- `mathematical_model::develop_refinement::develop_peer_ban_can_miss_checked_out_remote_work` (T1, T2, T10)
- `mathematical_model::properties::model_peer_ban_fence_is_bounded_and_expires_in_the_monotonic_clock_domain` (T3, T8, T10)
- `mathematical_model::properties::model_peer_ban_releases_only_matching_pre_authority_relay_handoffs` (T1, T7, T10)
- `mathematical_model::properties::model_peer_ban_removes_only_retained_remote_owners_and_releases_refetch` (T1, T3, T6, T7, T10)
- `mathematical_model::properties::model_remote_expiry_is_bounded_canonical_and_ignores_promoted_residency` (T1, T3, T6, T7, T8, T10)
- `mathematical_model::properties::model_wall_clock_rollback_never_fabricates_expiry_progress` (T3, T8, T10)

Cross-crate Rust evidence:

Generated cross-crate command: `cargo nextest run -p ckb-sync -E 'test(=relayer::tests::tx_verification_results::rejected_tx_can_be_requested_again_from_another_peer)'`

- `sync/src/relayer/tests/tx_verification_results.rs::relayer::tests::tx_verification_results::rejected_tx_can_be_requested_again_from_another_peer` (T7, T10) - A generation reset followed by delayed Remote ingress is closed by the later exact Reject, so the same raw transaction becomes requestable from another peer.

Process-level evidence:

- `truncate-clear-order`: `test/src/specs/rpc/truncate.rs::RpcTruncate` (T1, T3, T6, T9, T10, T12) - A clear issued by truncate after the chain transition is ordered after detached recovery and leaves one empty generation at the truncated snapshot. Paired units: `service::controller::tests::generation_clear_cannot_overtake_a_prior_chain_transition`, `authority::tests::foundation::uak_runtime_clear_scopes_and_snapshot_pairing_are_indivisible`. Command: `make integration CKB_TEST_ARGS='-c 1 RpcTruncate'`

#### `TP-EFFECT-001` - Atomic bounded effects and publication

Generated focused command: `cargo nextest run -p ckb-tx-pool --features internal -E 'test(=authority::tests::boundary_refinement::uak_effect_relay_boundaries_refine_publication_and_local_circuit_disposal) | test(=authority::tests::effect::uak_effect_full_preserves_ready_owner_and_charge) | test(=authority::tests::effect::uak_effect_receipt_keeps_the_committed_head_stable_across_producer_applies) | test(=authority::tests::effect::uak_effect_receipt_preserves_sequence_and_charge) | test(=authority::tests::effect::uak_effect_settlement_rejects_forged_source_sequence_and_cursor_without_mutation) | test(=authority::tests::effect::uak_generation_reset_coalesces_and_retain_never_resurrects_an_old_reset) | test(=authority::tests::effect::uak_production_effect_sizing_constructively_covers_non_rebuildable_shapes) | test(=authority::tests::effect::uak_remote_missing_wait_and_parent_request_share_one_backpressured_apply) | test(=authority::tests::ingress_batch::uak_retained_ingress_batch_commits_only_the_longest_effect_prefix) | test(=authority::tests::ingress_batch::uak_retained_ingress_batch_effect_cut_keeps_only_prior_owner_mutations) | test(=authority::tests::progress_refinement::uak_effect_publication_is_one_log_owned_three_way_observation) | test(=authority::tests::publisher::uak_cancelled_publisher_settles_its_tentative_cursor_to_the_fifo_head) | test(=authority::tests::publisher::uak_publisher_relay_disconnect_disposes_and_drains_the_authority_head) | test(=authority::tests::refinement::uak_partitioned_effect_pressure_refines_source_control) | test(=mathematical_model::adversarial_properties::model_full_effect_partition_stops_ready_before_any_authority_mutation) | test(=mathematical_model::boundary_trace::tests::boundary_effect_settlement_is_independent_of_external_relay_availability) | test(=mathematical_model::composition_properties::model_effect_capacity_serves_the_oldest_effectful_capability_before_newer_priority_work) | test(=mathematical_model::composition_properties::model_effect_pressure_retries_the_bounded_finished_slot_after_capacity_frees) | test(=mathematical_model::composition_properties::model_ready_prefix_stops_at_the_first_effect_control_class_boundary) | test(=mathematical_model::develop_refinement::develop_membership_commit_can_be_observed_without_a_required_effect) | test(=mathematical_model::progress_properties::model_effect_publication_observation_is_one_total_three_way_cut) | test(=mathematical_model::progress_properties::model_effect_wait_names_the_only_releaser_and_terminal_observation) | test(=mathematical_model::properties::model_drain_cannot_drop_a_committed_effect_or_its_unique_claim) | test(=mathematical_model::properties::model_effect_capacity_wait_is_mutation_free_and_keeps_the_ready_owner) | test(=mathematical_model::properties::model_effect_claim_remains_bound_to_the_head_across_later_commits) | test(=mathematical_model::properties::model_effect_payload_bytes_can_saturate_before_record_count) | test(=mathematical_model::properties::model_effect_regions_preserve_trusted_and_critical_headroom) | test(=mathematical_model::properties::model_endpoint_timeout_allows_at_most_one_detached_foreign_call) | test(=mathematical_model::properties::model_generation_reset_claim_is_superseded_without_mutating_authority) | test(=mathematical_model::properties::model_partially_settled_effect_batch_retains_its_full_capacity_charge) | test(=mathematical_model::properties::model_startup_effect_bound_covers_mixed_payload_and_dependency_publication) | test(=mathematical_model::properties::model_startup_rejects_an_effect_partition_smaller_than_one_indivisible_batch)'`

Rust evidence:

- `authority::tests::boundary_refinement::uak_effect_relay_boundaries_refine_publication_and_local_circuit_disposal` (T2, T7, T10)
- `authority::tests::effect::uak_effect_full_preserves_ready_owner_and_charge` (T1, T3, T6, T7)
- `authority::tests::effect::uak_effect_receipt_keeps_the_committed_head_stable_across_producer_applies` (T2, T3, T7, T10)
- `authority::tests::effect::uak_effect_receipt_preserves_sequence_and_charge` (T2, T3, T7)
- `authority::tests::effect::uak_effect_settlement_rejects_forged_source_sequence_and_cursor_without_mutation` (T2, T6, T7)
- `authority::tests::effect::uak_generation_reset_coalesces_and_retain_never_resurrects_an_old_reset` (T7, T10)
- `authority::tests::effect::uak_production_effect_sizing_constructively_covers_non_rebuildable_shapes` (T3, T7, T8)
- `authority::tests::effect::uak_remote_missing_wait_and_parent_request_share_one_backpressured_apply` (T4, T7, T10)
- `authority::tests::ingress_batch::uak_retained_ingress_batch_commits_only_the_longest_effect_prefix` (T3, T6, T7, T8)
- `authority::tests::ingress_batch::uak_retained_ingress_batch_effect_cut_keeps_only_prior_owner_mutations` (T1, T3, T6, T7, T8)
- `authority::tests::progress_refinement::uak_effect_publication_is_one_log_owned_three_way_observation` (T2, T7, T10)
- `authority::tests::publisher::uak_cancelled_publisher_settles_its_tentative_cursor_to_the_fifo_head` (T2, T7, T10)
- `authority::tests::publisher::uak_publisher_relay_disconnect_disposes_and_drains_the_authority_head` (T7, T10)
- `authority::tests::refinement::uak_partitioned_effect_pressure_refines_source_control` (T3, T6, T7, T8)
- `mathematical_model::adversarial_properties::model_full_effect_partition_stops_ready_before_any_authority_mutation` (T3, T6, T7, T8)
- `mathematical_model::boundary_trace::tests::boundary_effect_settlement_is_independent_of_external_relay_availability` (T2, T7, T10)
- `mathematical_model::composition_properties::model_effect_capacity_serves_the_oldest_effectful_capability_before_newer_priority_work` (T2, T3, T6, T7, T8, T10)
- `mathematical_model::composition_properties::model_effect_pressure_retries_the_bounded_finished_slot_after_capacity_frees` (T2, T3, T6, T7, T8, T10)
- `mathematical_model::composition_properties::model_ready_prefix_stops_at_the_first_effect_control_class_boundary` (T3, T6, T7, T8)
- `mathematical_model::develop_refinement::develop_membership_commit_can_be_observed_without_a_required_effect` (T6, T7)
- `mathematical_model::progress_properties::model_effect_publication_observation_is_one_total_three_way_cut` (T2, T7, T10)
- `mathematical_model::progress_properties::model_effect_wait_names_the_only_releaser_and_terminal_observation` (T2, T7, T10)
- `mathematical_model::properties::model_drain_cannot_drop_a_committed_effect_or_its_unique_claim` (T2, T7, T10)
- `mathematical_model::properties::model_effect_capacity_wait_is_mutation_free_and_keeps_the_ready_owner` (T1, T3, T6, T7, T8)
- `mathematical_model::properties::model_effect_claim_remains_bound_to_the_head_across_later_commits` (T2, T7, T10)
- `mathematical_model::properties::model_effect_payload_bytes_can_saturate_before_record_count` (T3, T6, T7, T8)
- `mathematical_model::properties::model_effect_regions_preserve_trusted_and_critical_headroom` (T3, T6, T7, T8)
- `mathematical_model::properties::model_endpoint_timeout_allows_at_most_one_detached_foreign_call` (T7, T8, T10)
- `mathematical_model::properties::model_generation_reset_claim_is_superseded_without_mutating_authority` (T2, T6, T7, T9)
- `mathematical_model::properties::model_partially_settled_effect_batch_retains_its_full_capacity_charge` (T3, T6, T7, T8)
- `mathematical_model::properties::model_startup_effect_bound_covers_mixed_payload_and_dependency_publication` (T3, T7, T8)
- `mathematical_model::properties::model_startup_rejects_an_effect_partition_smaller_than_one_indivisible_batch` (T3, T7, T8)

Cross-crate Rust evidence:

Generated cross-crate command: `cargo nextest run -p ckb-sync -E 'test(=relayer::tests::tx_verification_results::committed_tx_result_is_consumed_without_relay_peers) | test(=relayer::tests::tx_verification_results::rejected_tx_can_be_requested_again_from_another_peer)'`

- `sync/src/relayer/tests/tx_verification_results.rs::relayer::tests::tx_verification_results::committed_tx_result_is_consumed_without_relay_peers` (T7, T10) - Committed tx-pool results update the local relay projection even with no connected relay peer and retain bounded later broadcast intent.
- `sync/src/relayer/tests/tx_verification_results.rs::relayer::tests::tx_verification_results::rejected_tx_can_be_requested_again_from_another_peer` (T7, T10) - A generation reset followed by delayed Remote ingress is closed by the later exact Reject, so the same raw transaction becomes requestable from another peer.

Process-level evidence:

- `multi-parent-orphan-frontier`: `test/src/specs/tx_pool/orphan_tx.rs::TxPoolOrphanReverse` (T4, T6, T7, T10) - A complete multi-parent missing frontier and its relay request commit in one bounded transition. Paired units: `authority::tests::dependency::uak_direct_parent_acceptance_publishes_output_availability_atomically`, `authority::tests::effect::uak_remote_missing_wait_and_parent_request_share_one_backpressured_apply`. Command: `make integration CKB_TEST_ARGS='-c 1 TxPoolOrphanReverse'`

#### `TP-REORG-001` - Reliable atomic chain reconciliation

Generated focused command: `cargo nextest run -p ckb-tx-pool --features internal -E 'test(=authority::tests::chain::uak_chain_boundary_closes_ordered_backpressure_without_open_plan_errors) | test(=authority::tests::chain::uak_chain_commit_removes_a_parent_without_stranding_its_surviving_child) | test(=authority::tests::chain::uak_chain_recovery_excludes_an_oversized_subtree_and_keeps_unrelated_work) | test(=authority::tests::chain::uak_detached_parent_and_accepted_child_recover_parent_first) | test(=authority::tests::chain::uak_runtime_chain_boundary_reconciles_indexed_gap_against_paired_snapshot) | test(=mathematical_model::adversarial_properties::model_hostile_trace_search_finds_the_shortest_same_tip_new_revision_schedule) | test(=mathematical_model::composition_properties::model_chain_race_retires_finished_evidence_and_rechecks_out_current_resolve_work) | test(=mathematical_model::develop_refinement::develop_concurrent_clear_can_be_overtaken_by_detached_recovery) | test(=mathematical_model::develop_refinement::develop_gap_and_detached_uncle_can_jointly_censor_reproposal) | test(=mathematical_model::properties::model_chain_advance_requeues_finished_work_and_retires_it_without_double_release) | test(=mathematical_model::properties::model_chain_conflict_closes_a_computing_dependency_tree_in_one_apply) | test(=mathematical_model::properties::model_chain_cut_allows_same_tip_progress_but_rejects_a_stale_receipt) | test(=mathematical_model::properties::model_chain_effect_plan_is_complete_canonical_and_atomic) | test(=mathematical_model::properties::model_chain_reconciliation_promotes_committed_parent_evidence_without_losing_child) | test(=mathematical_model::properties::model_chain_recovery_skips_an_excluded_root_and_descendants_but_keeps_unrelated_work) | test(=mathematical_model::properties::model_chain_window_demotes_remote_base_and_expires_trusted_proposal_atomically) | test(=mathematical_model::properties::model_detached_chain_cell_recovers_the_complete_accepted_causal_closure) | test(=mathematical_model::properties::model_detached_header_recovers_accepted_consumer_without_public_rejection) | test(=mathematical_model::properties::model_gap_is_demoted_to_pending_when_the_new_window_contains_no_proposal) | test(=mathematical_model::properties::model_generation_replacement_clears_owners_and_leaves_only_bounded_stale_capabilities) | test(=mathematical_model::properties::model_queued_verify_evidence_is_lazily_requeued_after_a_chain_advance) | test(=service::controller::tests::authoritative_reorg_delivery_is_independent_of_rpc_readiness) | test(=service::controller::tests::closed_reorg_consumer_fails_without_waiting) | test(=service::controller::tests::generation_clear_cannot_overtake_a_prior_chain_transition)'`

Rust evidence:

- `authority::tests::chain::uak_chain_boundary_closes_ordered_backpressure_without_open_plan_errors` (T6, T9, T10)
- `authority::tests::chain::uak_chain_commit_removes_a_parent_without_stranding_its_surviving_child` (T4, T10)
- `authority::tests::chain::uak_chain_recovery_excludes_an_oversized_subtree_and_keeps_unrelated_work` (T1, T3, T4, T6, T8, T9, T10)
- `authority::tests::chain::uak_detached_parent_and_accepted_child_recover_parent_first` (T1, T4, T6, T9)
- `authority::tests::chain::uak_runtime_chain_boundary_reconciles_indexed_gap_against_paired_snapshot` (T5, T6, T9, T12)
- `mathematical_model::adversarial_properties::model_hostile_trace_search_finds_the_shortest_same_tip_new_revision_schedule` (T2, T9, T10, T11)
- `mathematical_model::composition_properties::model_chain_race_retires_finished_evidence_and_rechecks_out_current_resolve_work` (T2, T6, T9, T10, T11)
- `mathematical_model::develop_refinement::develop_concurrent_clear_can_be_overtaken_by_detached_recovery` (T1, T6, T9, T10)
- `mathematical_model::develop_refinement::develop_gap_and_detached_uncle_can_jointly_censor_reproposal` (T5, T9, T12, T13)
- `mathematical_model::properties::model_chain_advance_requeues_finished_work_and_retires_it_without_double_release` (T1, T2, T3, T6, T8, T9, T10, T11)
- `mathematical_model::properties::model_chain_conflict_closes_a_computing_dependency_tree_in_one_apply` (T1, T2, T4, T6, T7, T9, T10)
- `mathematical_model::properties::model_chain_cut_allows_same_tip_progress_but_rejects_a_stale_receipt` (T2, T6, T9, T10, T11)
- `mathematical_model::properties::model_chain_effect_plan_is_complete_canonical_and_atomic` (T1, T3, T4, T6, T7, T9, T10)
- `mathematical_model::properties::model_chain_reconciliation_promotes_committed_parent_evidence_without_losing_child` (T1, T4, T6, T9, T10, T11)
- `mathematical_model::properties::model_chain_recovery_skips_an_excluded_root_and_descendants_but_keeps_unrelated_work` (T1, T3, T4, T6, T8, T9, T10)
- `mathematical_model::properties::model_chain_window_demotes_remote_base_and_expires_trusted_proposal_atomically` (T1, T3, T4, T6, T7, T9, T10)
- `mathematical_model::properties::model_detached_chain_cell_recovers_the_complete_accepted_causal_closure` (T1, T3, T4, T6, T9, T10)
- `mathematical_model::properties::model_detached_header_recovers_accepted_consumer_without_public_rejection` (T1, T3, T4, T6, T9, T10, T11)
- `mathematical_model::properties::model_gap_is_demoted_to_pending_when_the_new_window_contains_no_proposal` (T5, T6, T9, T12, T13)
- `mathematical_model::properties::model_generation_replacement_clears_owners_and_leaves_only_bounded_stale_capabilities` (T1, T2, T3, T6, T8, T9)
- `mathematical_model::properties::model_queued_verify_evidence_is_lazily_requeued_after_a_chain_advance` (T2, T6, T8, T9, T10, T11)
- `service::controller::tests::authoritative_reorg_delivery_is_independent_of_rpc_readiness` (T9, T10)
- `service::controller::tests::closed_reorg_consumer_fails_without_waiting` (T9, T10)
- `service::controller::tests::generation_clear_cannot_overtake_a_prior_chain_transition` (T1, T9, T10)

Cross-crate Rust evidence:

Generated cross-crate command: `cargo nextest run -p ckb-sync -E 'test(=tests::sync_shared::test_insert_new_block)'`

- `sync/src/tests/sync_shared.rs::tests::sync_shared::test_insert_new_block` (T9, T10) - A chain-only fixture explicitly retires the dormant tx-pool consumer, so reliable best-tip delivery cannot become an unowned capacity-one wait.

Process-level evidence:

- `accepted-descendants-after-parent-reorg`: `test/src/specs/tx_pool/pool_reconcile.rs::PoolResolveConflictAfterReorg` (T1, T3, T4, T6, T9, T10) - A detached accepted parent and surviving descendant closure reconcile as one valid owner generation. Paired units: `authority::tests::dependency::uak_parent_terminalization_cannot_strand_trusted_child`, `authority::tests::chain::uak_detached_parent_and_accepted_child_recover_parent_first`, `authority::tests::foundation::uak_capacity_eviction_removes_one_complete_causal_component`. Command: `make integration CKB_TEST_ARGS='-c 1 PoolResolveConflictAfterReorg'`
- `async-uncle-candidate-publication`: `test/src/specs/mining/uncle.rs::UncleInheritFromForkUncle` (T8, T9, T10, T13) - Candidate-uncles remain asynchronously publishable without violating reset/full priority or proposal conflict filtering. Paired units: `service::controller::tests::authoritative_reorg_delivery_is_independent_of_rpc_readiness`, `authority::tests::template_driver::uak_template_driver_full_priority_and_partial_occ_use_one_output_revision`, `block_assembler::tests::optional_content_uses_one_budget_and_filters_only_published_conflicts`. Command: `make integration CKB_TEST_ARGS='-c 1 UncleInheritFromForkUncle'`
- `dependent-reorg-chain`: `test/src/specs/tx_pool/reorg_recovers_dependent.rs::ReorgRecoversDependentChain` (T1, T4, T6, T9, T10) - Detached dependent transactions recover parent-first without a lost dependency wake. Paired units: `authority::tests::dependency::uak_parent_terminalization_cannot_strand_trusted_child`, `authority::tests::chain::uak_detached_parent_and_accepted_child_recover_parent_first`. Command: `make integration CKB_TEST_ARGS='-c 1 ReorgRecoversDependentChain'`
- `dependent-reorg-transactions`: `test/src/specs/tx_pool/reorg_recovers_dependent.rs::ReorgRecoversDependentTxs` (T1, T4, T6, T9, T10) - Ordinary detached transaction recovery preserves dependency order and exact chain generation. Paired units: `authority::tests::dependency::uak_parent_terminalization_cannot_strand_trusted_child`, `authority::tests::chain::uak_detached_parent_and_accepted_child_recover_parent_first`. Command: `make integration CKB_TEST_ARGS='-c 1 ReorgRecoversDependentTxs'`
- `normal-mining-reorg-tree`: `test/src/specs/tx_pool/reorg_recovers_dependent.rs::ReorgRecoversDependentPendingTree` (T5, T9, T10, T12, T13) - A recovered dependent tree is re-proposed and mined through the normal template path after reorg. Paired units: `authority::tests::chain::uak_runtime_chain_boundary_reconciles_indexed_gap_against_paired_snapshot`, `authority::tests::template_driver::uak_template_driver_full_priority_and_partial_occ_use_one_output_revision`. Command: `make integration CKB_TEST_ARGS='-c 1 ReorgRecoversDependentPendingTree'`
- `truncate-clear-order`: `test/src/specs/rpc/truncate.rs::RpcTruncate` (T1, T3, T6, T9, T10, T12) - A clear issued by truncate after the chain transition is ordered after detached recovery and leaves one empty generation at the truncated snapshot. Paired units: `service::controller::tests::generation_clear_cannot_overtake_a_prior_chain_transition`, `authority::tests::foundation::uak_runtime_clear_scopes_and_snapshot_pairing_are_indivisible`. Command: `make integration CKB_TEST_ARGS='-c 1 RpcTruncate'`

#### `TP-PERSIST-001` - Coherent bounded persistence

Generated focused command: `cargo nextest run -p ckb-tx-pool --features internal -E 'test(=authority::tests::read::uak_persistence_receipt_is_coherent_and_parent_first) | test(=authority::tests::service::uak_service_persists_one_coherent_authority_receipt_outside_the_guard) | test(=authority::tests::topology::uak_topology_clean_shutdown_drains_effects_before_persistence) | test(=authority::tests::topology::uak_topology_shutdown_stops_the_paired_verification_generation) | test(=mathematical_model::properties::model_initialization_replay_failure_drains_without_publishing_partial_authority) | test(=mathematical_model::properties::model_persistence_contains_only_accepted_and_recovery_retained_owners) | test(=mathematical_model::properties::model_shutdown_orders_capability_drain_before_persistence) | test(=mathematical_model::properties::model_shutdown_rejects_early_persistence_and_distinguishes_derived_failure) | test(=persisted::tests::persistence_loader_accepts_legacy_v1_vector) | test(=persisted::tests::persistence_loader_rejects_an_unrepresentable_read_bound_before_io) | test(=persisted::tests::persistence_v2_rejects_oversized_file_before_reading_payload) | test(=persisted::tests::persistence_writer_admits_only_one_snapshot_owner)'`

Rust evidence:

- `authority::tests::read::uak_persistence_receipt_is_coherent_and_parent_first` (T4, T9, T12)
- `authority::tests::service::uak_service_persists_one_coherent_authority_receipt_outside_the_guard` (T9, T10, T12)
- `authority::tests::topology::uak_topology_clean_shutdown_drains_effects_before_persistence` (T2, T7, T9, T10, T12)
- `authority::tests::topology::uak_topology_shutdown_stops_the_paired_verification_generation` (T2, T9, T10)
- `mathematical_model::properties::model_initialization_replay_failure_drains_without_publishing_partial_authority` (T1, T7, T9, T10, T12)
- `mathematical_model::properties::model_persistence_contains_only_accepted_and_recovery_retained_owners` (T4, T9, T11, T12)
- `mathematical_model::properties::model_shutdown_orders_capability_drain_before_persistence` (T1, T2, T7, T9, T10, T12)
- `mathematical_model::properties::model_shutdown_rejects_early_persistence_and_distinguishes_derived_failure` (T2, T6, T7, T9, T10, T12)
- `persisted::tests::persistence_loader_accepts_legacy_v1_vector` (T11, T12)
- `persisted::tests::persistence_loader_rejects_an_unrepresentable_read_bound_before_io` (T8, T11)
- `persisted::tests::persistence_v2_rejects_oversized_file_before_reading_payload` (T8, T11)
- `persisted::tests::persistence_writer_admits_only_one_snapshot_owner` (T10, T12)

#### `TP-QUERY-001` - Coherent public projections

Generated focused command: `cargo nextest run -p ckb-tx-pool --features internal -E 'test(=authority::tests::query::uak_compact_receipt_releases_authority_before_storage_lookup) | test(=authority::tests::query::uak_fixed_output_full_queries_do_not_grow_row_scratch) | test(=authority::tests::query::uak_full_query_gate_does_not_serialize_point_status_reads) | test(=authority::tests::query::uak_full_query_scratch_makes_strict_progress_across_repeated_growth) | test(=authority::tests::query::uak_full_query_scratch_rejects_bounds_and_allocation_failure) | test(=authority::tests::query::uak_owned_pool_queries_share_one_status_and_aggregate_cut) | test(=authority::tests::query::uak_owned_transaction_query_hides_replacement_history_and_reports_minimum_fee) | test(=authority::tests::query::uak_status_and_detail_queries_isolate_optional_replacement_fee_overflow) | test(=authority::tests::read::uak_query_never_splices_two_authority_cuts) | test(=mathematical_model::properties::model_optional_query_arithmetic_failure_never_invalidates_the_authority_result) | test(=mathematical_model::properties::model_query_cost_keeps_concurrency_scan_sort_and_output_terms_explicit) | test(=mathematical_model::properties::model_query_projection_collapses_only_the_documented_internal_states) | test(=mathematical_model::topology_properties::model_prepared_query_scratch_is_the_only_zero_duplicate_root_fix) | test(=mathematical_model::topology_properties::model_query_cost_uses_the_full_declared_u64_domain) | test(=mathematical_model::topology_properties::model_query_scratch_growth_has_a_finite_strict_rank)'`

Rust evidence:

- `authority::tests::query::uak_compact_receipt_releases_authority_before_storage_lookup` (T10, T12)
- `authority::tests::query::uak_fixed_output_full_queries_do_not_grow_row_scratch` (T8, T10)
- `authority::tests::query::uak_full_query_gate_does_not_serialize_point_status_reads` (T8, T10, T12)
- `authority::tests::query::uak_full_query_scratch_makes_strict_progress_across_repeated_growth` (T8, T10)
- `authority::tests::query::uak_full_query_scratch_rejects_bounds_and_allocation_failure` (T6, T8, T10)
- `authority::tests::query::uak_owned_pool_queries_share_one_status_and_aggregate_cut` (T12)
- `authority::tests::query::uak_owned_transaction_query_hides_replacement_history_and_reports_minimum_fee` (T12)
- `authority::tests::query::uak_status_and_detail_queries_isolate_optional_replacement_fee_overflow` (T6, T8, T12)
- `authority::tests::read::uak_query_never_splices_two_authority_cuts` (T9, T12)
- `mathematical_model::properties::model_optional_query_arithmetic_failure_never_invalidates_the_authority_result` (T6, T8, T12)
- `mathematical_model::properties::model_query_cost_keeps_concurrency_scan_sort_and_output_terms_explicit` (T8, T10, T12)
- `mathematical_model::properties::model_query_projection_collapses_only_the_documented_internal_states` (T5, T6, T8, T12)
- `mathematical_model::topology_properties::model_prepared_query_scratch_is_the_only_zero_duplicate_root_fix` (T6, T8, T12)
- `mathematical_model::topology_properties::model_query_cost_uses_the_full_declared_u64_domain` (T6, T8, T12)
- `mathematical_model::topology_properties::model_query_scratch_growth_has_a_finite_strict_rank` (T8, T10, T12)

Process-level evidence:

- `failed-rbf-terminal`: `test/src/specs/tx_pool/replace.rs::RbfContainInvalidInput` (T1, T3, T6, T7, T8, T12) - A policy-rejected RBF candidate publishes per-hash rejection while preserving every accepted owner and creating no uncharged conflict-history residency. Paired units: `authority::tests::foundation::uak_terminal_outcome_and_effect_commit_together`, `authority::tests::foundation::uak_failed_rbf_fee_disposition_preserves_victims_and_terminalizes_candidate`, `authority::tests::foundation::uak_independent_rbf_churn_never_exceeds_replacement_history_budget`, `authority::tests::query::uak_owned_transaction_query_hides_replacement_history_and_reports_minimum_fee`. Command: `make integration CKB_TEST_ARGS='-c 1 RbfContainInvalidInput'`

#### `TP-DEFECT-001` - Rust-native ordinary failure boundary

Generated focused command: `cargo nextest run -p ckb-tx-pool --features internal -E 'test(=authority::tests::contract_observation_refinement::uak_operational_metrics_are_the_exact_read_only_owned_counter_projection) | test(=authority::tests::contract_observation_refinement::uak_plan_scratch_reservations_observe_capacity_without_authority_mutation) | test(=authority::tests::contract_observation_refinement::uak_remote_expiry_removes_exactly_the_effect_bounded_due_prefix) | test(=authority::tests::foundation::uak_allocation_failure_discards_result_without_retaining_compute_capability) | test(=authority::tests::foundation::uak_counter_exhaustion_is_typed_and_mutation_free) | test(=authority::tests::ingress::uak_retained_ingress_boundary_keeps_legal_pressure_out_of_fail_stop) | test(=authority::tests::ingress_batch::uak_retained_ingress_batch_keeps_recovery_payload_variant_unchanged) | test(=authority::tests::resolver::uak_duplicate_inputs_are_a_malformed_outcome_not_an_authority_fault) | test(=authority::tests::service::uak_candidate_uncle_degradation_remains_outside_authority_invalidity) | test(=authority::tests::service::uak_only_integrity_faults_invalidate_a_generation) | test(=authority::tests::service::uak_operational_failure_classes_follow_ownership_boundaries) | test(=authority::tests::service::uak_ordered_chain_update_has_no_droppable_operational_error) | test(=authority::tests::service::uak_recent_reject_encoding_failure_remains_outside_authority_invalidity) | test(=authority::tests::topology::uak_topology_forbids_persistence_only_after_authority_integrity_loss) | test(=block_assembler::candidate_uncles::tests::candidate_uncle_source_version_exhaustion_is_typed_and_mutation_free) | test(=mathematical_model::contract_observation_properties::model_expiry_index_producer_is_the_exact_equivalence_premise) | test(=mathematical_model::contract_observation_properties::model_operational_metrics_projection_preserves_every_owned_counter) | test(=mathematical_model::contract_observation_properties::model_successful_reservation_and_bounded_prefix_have_exact_observations) | test(=mathematical_model::properties::model_allocation_pressure_is_an_ordinary_terminal_outcome_not_a_timer_retry) | test(=mathematical_model::properties::model_counter_exhaustion_is_a_mutation_free_ordinary_outcome) | test(=mathematical_model::properties::model_payload_variant_is_an_ordinary_outcome_and_same_witness_promotion_is_atomic)'`

Rust evidence:

- `authority::tests::contract_observation_refinement::uak_operational_metrics_are_the_exact_read_only_owned_counter_projection` (T3, T6, T8, T10)
- `authority::tests::contract_observation_refinement::uak_plan_scratch_reservations_observe_capacity_without_authority_mutation` (T3, T6, T8, T10)
- `authority::tests::contract_observation_refinement::uak_remote_expiry_removes_exactly_the_effect_bounded_due_prefix` (T3, T6, T8, T10)
- `authority::tests::foundation::uak_allocation_failure_discards_result_without_retaining_compute_capability` (T3, T6, T8, T10)
- `authority::tests::foundation::uak_counter_exhaustion_is_typed_and_mutation_free` (T2, T6, T8)
- `authority::tests::ingress::uak_retained_ingress_boundary_keeps_legal_pressure_out_of_fail_stop` (T3, T8)
- `authority::tests::ingress_batch::uak_retained_ingress_batch_keeps_recovery_payload_variant_unchanged` (T1, T3, T6, T11)
- `authority::tests::resolver::uak_duplicate_inputs_are_a_malformed_outcome_not_an_authority_fault` (T8, T11)
- `authority::tests::service::uak_candidate_uncle_degradation_remains_outside_authority_invalidity` (T6, T10, T13)
- `authority::tests::service::uak_only_integrity_faults_invalidate_a_generation` (T6, T7, T10)
- `authority::tests::service::uak_operational_failure_classes_follow_ownership_boundaries` (T7, T10)
- `authority::tests::service::uak_ordered_chain_update_has_no_droppable_operational_error` (T6, T9, T10)
- `authority::tests::service::uak_recent_reject_encoding_failure_remains_outside_authority_invalidity` (T7, T10)
- `authority::tests::topology::uak_topology_forbids_persistence_only_after_authority_integrity_loss` (T6, T7, T10)
- `block_assembler::candidate_uncles::tests::candidate_uncle_source_version_exhaustion_is_typed_and_mutation_free` (T6, T8, T13)
- `mathematical_model::contract_observation_properties::model_expiry_index_producer_is_the_exact_equivalence_premise` (T3, T6, T8, T10)
- `mathematical_model::contract_observation_properties::model_operational_metrics_projection_preserves_every_owned_counter` (T3, T6, T8, T10)
- `mathematical_model::contract_observation_properties::model_successful_reservation_and_bounded_prefix_have_exact_observations` (T3, T6, T8, T10)
- `mathematical_model::properties::model_allocation_pressure_is_an_ordinary_terminal_outcome_not_a_timer_retry` (T6, T8, T10)
- `mathematical_model::properties::model_counter_exhaustion_is_a_mutation_free_ordinary_outcome` (T2, T6, T8)
- `mathematical_model::properties::model_payload_variant_is_an_ordinary_outcome_and_same_witness_promotion_is_atomic` (T1, T6, T11)

Process-level evidence:

- `local-test-rpc-direct`: `test/src/specs/tx_pool/local_test_submission.rs::LocalTestSubmissionIsDirect` (T1, T2, T3, T6, T7, T8, T11) - Local admission remains direct and atomic while TestAccept remains read-only under the same policy. Paired units: `authority::tests::foundation::uak_direct_local_admission_moves_from_absent_to_accepted_in_one_apply`, `authority::tests::chain::uak_final_admission_receipt_is_stale_after_chain_view_aba`, `authority::tests::resolver::uak_duplicate_inputs_are_a_malformed_outcome_not_an_authority_fault`. Command: `make integration CKB_TEST_ARGS='-c 1 LocalTestSubmissionIsDirect'`

#### `TP-POOL-001` - Atomic accepted membership graph

Generated focused command: `cargo nextest run -p ckb-tx-pool --features internal -E 'test(=authority::tests::foundation::uak_capacity_eviction_removes_one_complete_causal_component) | test(=authority::tests::foundation::uak_causal_diamond_is_projection_equivalent_for_every_arrival_order) | test(=authority::tests::foundation::uak_membership_projects_one_spender_and_one_causal_graph) | test(=authority::tests::foundation::uak_status_reconcile_updates_count_and_eviction_projection_once) | test(=authority::tests::refinement::uak_eviction_order_refines_the_exact_ckb_weight_and_tuple) | test(=mathematical_model::eviction_properties::model_eviction_key_is_the_exact_status_rate_count_arrival_identity_order) | test(=mathematical_model::eviction_properties::model_eviction_uses_the_stronger_of_self_and_descendant_fee_rate) | test(=mathematical_model::eviction_properties::model_eviction_weight_and_fee_rate_preserve_ckb_rounding_and_saturation)'`

Rust evidence:

- `authority::tests::foundation::uak_capacity_eviction_removes_one_complete_causal_component` (T3, T4, T6, T8)
- `authority::tests::foundation::uak_causal_diamond_is_projection_equivalent_for_every_arrival_order` (T4, T6)
- `authority::tests::foundation::uak_membership_projects_one_spender_and_one_causal_graph` (T1, T4, T6, T12)
- `authority::tests::foundation::uak_status_reconcile_updates_count_and_eviction_projection_once` (T5, T6, T12)
- `authority::tests::refinement::uak_eviction_order_refines_the_exact_ckb_weight_and_tuple` (T3, T5, T6, T8, T12)
- `mathematical_model::eviction_properties::model_eviction_key_is_the_exact_status_rate_count_arrival_identity_order` (T5, T6, T8, T11)
- `mathematical_model::eviction_properties::model_eviction_uses_the_stronger_of_self_and_descendant_fee_rate` (T3, T5, T6, T8)
- `mathematical_model::eviction_properties::model_eviction_weight_and_fee_rate_preserve_ckb_rounding_and_saturation` (T5, T6, T8, T11)

Process-level evidence:

- `accepted-descendants-after-parent-reorg`: `test/src/specs/tx_pool/pool_reconcile.rs::PoolResolveConflictAfterReorg` (T1, T3, T4, T6, T9, T10) - A detached accepted parent and surviving descendant closure reconcile as one valid owner generation. Paired units: `authority::tests::dependency::uak_parent_terminalization_cannot_strand_trusted_child`, `authority::tests::chain::uak_detached_parent_and_accepted_child_recover_parent_first`, `authority::tests::foundation::uak_capacity_eviction_removes_one_complete_causal_component`. Command: `make integration CKB_TEST_ARGS='-c 1 PoolResolveConflictAfterReorg'`
- `conditional-reader-fanout`: `test/src/specs/tx_pool/limit.rs::TxPoolLimitAncestorCount` (T3, T4, T6, T8, T10, T13) - A high-fanout dependency shape remains bounded without corrupting accepted causal limits. Paired units: `authority::tests::dependency::uak_popular_dependency_maintenance_has_one_edge_steps_and_key_fairness`, `authority::tests::foundation::uak_capacity_eviction_removes_one_complete_causal_component`, `block_assembler::candidate_uncles::tests::full_container_keeps_a_hard_global_bound`. Command: `make integration CKB_TEST_ARGS='-c 1 TxPoolLimitAncestorCount'`
- `rbf-cell-deps`: `test/src/specs/tx_pool/replace.rs::RbfCellDepsCheck` (T1, T4, T6, T11, T12) - A replacement depending on any victim is rejected before graph mutation. Paired units: `authority::tests::foundation::uak_rbf_dependency_on_any_victim_is_mutation_free`, `authority::tests::foundation::uak_membership_projects_one_spender_and_one_causal_graph`. Command: `make integration CKB_TEST_ARGS='-c 1 RbfCellDepsCheck'`

#### `TP-TEMPLATE-001` - Concurrent versioned template convergence

Generated focused command: `cargo nextest run -p ckb-tx-pool --features internal -E 'test(=authority::tests::packing::uak_template_packer_selects_an_exact_fit_cpfp_package_parent_first) | test(=authority::tests::template::uak_recovered_tree_has_normal_template_proposal_path) | test(=authority::tests::template::uak_template_read_receipt_shares_order_and_complete_resolved_payload) | test(=authority::tests::template_driver::uak_template_driver_full_priority_and_partial_occ_use_one_output_revision) | test(=authority::tests::template_driver::uak_template_source_probe_skips_irrelevant_population_captures) | test(=block_assembler::tests::block_template_preserves_consensus_uncle_limit_above_u8) | test(=block_assembler::tests::optional_content_uses_one_budget_and_filters_only_published_conflicts) | test(=mathematical_model::properties::model_derived_publication_is_versioned_and_failure_cannot_mutate_authority) | test(=mathematical_model::properties::model_template_filters_candidate_uncles_that_would_censor_current_proposals) | test(=mathematical_model::properties::model_template_full_preempts_reset_without_serializing_optimistic_lanes)'`

Rust evidence:

- `authority::tests::packing::uak_template_packer_selects_an_exact_fit_cpfp_package_parent_first` (T4, T8, T13)
- `authority::tests::template::uak_recovered_tree_has_normal_template_proposal_path` (T4, T9, T10, T13)
- `authority::tests::template::uak_template_read_receipt_shares_order_and_complete_resolved_payload` (T4, T9, T12, T13)
- `authority::tests::template_driver::uak_template_driver_full_priority_and_partial_occ_use_one_output_revision` (T9, T10, T13)
- `authority::tests::template_driver::uak_template_source_probe_skips_irrelevant_population_captures` (T10, T13)
- `block_assembler::tests::block_template_preserves_consensus_uncle_limit_above_u8` (T8, T11, T13)
- `block_assembler::tests::optional_content_uses_one_budget_and_filters_only_published_conflicts` (T8, T13)
- `mathematical_model::properties::model_derived_publication_is_versioned_and_failure_cannot_mutate_authority` (T6, T7, T10, T12, T13)
- `mathematical_model::properties::model_template_filters_candidate_uncles_that_would_censor_current_proposals` (T8, T9, T10, T13)
- `mathematical_model::properties::model_template_full_preempts_reset_without_serializing_optimistic_lanes` (T9, T10, T13)

Process-level evidence:

- `async-uncle-candidate-publication`: `test/src/specs/mining/uncle.rs::UncleInheritFromForkUncle` (T8, T9, T10, T13) - Candidate-uncles remain asynchronously publishable without violating reset/full priority or proposal conflict filtering. Paired units: `service::controller::tests::authoritative_reorg_delivery_is_independent_of_rpc_readiness`, `authority::tests::template_driver::uak_template_driver_full_priority_and_partial_occ_use_one_output_revision`, `block_assembler::tests::optional_content_uses_one_budget_and_filters_only_published_conflicts`. Command: `make integration CKB_TEST_ARGS='-c 1 UncleInheritFromForkUncle'`
- `cell-dep-arrival-order`: `test/src/specs/tx_pool/dead_cell_deps.rs::CellBeingCellDepAndSpentInSameBlockTestGetBlockTemplate` (T4, T6, T11, T12, T13) - A selected cell-dep reader remains ordered before the spender regardless of arrival order. Paired units: `authority::tests::foundation::uak_coupled_membership_requires_exact_positive_input_evidence`, `authority::tests::template::uak_template_read_receipt_shares_order_and_complete_resolved_payload`. Command: `make integration CKB_TEST_ARGS='-c 1 CellBeingCellDepAndSpentInSameBlockTestGetBlockTemplate'`
- `normal-mining-reorg-tree`: `test/src/specs/tx_pool/reorg_recovers_dependent.rs::ReorgRecoversDependentPendingTree` (T5, T9, T10, T12, T13) - A recovered dependent tree is re-proposed and mined through the normal template path after reorg. Paired units: `authority::tests::chain::uak_runtime_chain_boundary_reconciles_indexed_gap_against_paired_snapshot`, `authority::tests::template_driver::uak_template_driver_full_priority_and_partial_occ_use_one_output_revision`. Command: `make integration CKB_TEST_ARGS='-c 1 ReorgRecoversDependentPendingTree'`
- `rbf-proposed-success`: `test/src/specs/tx_pool/replace.rs::RbfReplaceProposedSuccess` (T1, T4, T6, T9, T10, T13) - A successful proposed replacement updates membership and template convergence once; committing its parent cannot wake retained victims while the winner still spends that output. Paired units: `authority::tests::chain::uak_chain_output_availability_respects_a_surviving_pool_spender`, `authority::tests::foundation::uak_rbf_replaces_the_complete_descendant_closure_atomically`, `authority::tests::template::uak_recovered_tree_has_normal_template_proposal_path`. Command: `make integration CKB_TEST_ARGS='-c 1 RbfReplaceProposedSuccess'`
- `rbf-proposed-template-refresh`: `test/src/specs/tx_pool/replace.rs::RbfRejectReplaceProposed` (T1, T4, T6, T9, T13) - Rejected proposed replacement is mutation-free and cannot publish a stale template generation. Paired units: `authority::tests::foundation::uak_rbf_replaces_the_complete_descendant_closure_atomically`, `authority::tests::template_driver::uak_template_driver_full_priority_and_partial_occ_use_one_output_revision`. Command: `make integration CKB_TEST_ARGS='-c 1 RbfRejectReplaceProposed'`

#### `TP-IDENTITY-001` - Exact transaction and evidence identity

Generated focused command: `cargo nextest run -p ckb-tx-pool --features internal -E 'test(=authority::tests::evidence_refinement::uak_direct_rejection_refines_the_closed_view_and_source_truth_table) | test(=authority::tests::evidence_refinement::uak_final_and_direct_acceptance_refine_current_chain_and_dependency_cuts) | test(=authority::tests::evidence_refinement::uak_final_subject_refines_every_reachable_currentness_outcome) | test(=authority::tests::evidence_refinement::uak_missing_dependency_policy_refines_source_and_parent_ownership) | test(=authority::tests::foundation::uak_short_id_collision_cannot_alias_primary_identity) | test(=authority::tests::resolver::uak_verification_cache_lookup_cannot_substitute_a_nearby_request) | test(=authority::tests::resolver::uak_verification_request_binds_environment_rules_and_witness_cache_key) | test(=authority::tests::validation::uak_final_validation_rejects_a_mixed_authority_snapshot_cut) | test(=authority::tests::validation::uak_same_tip_unproven_location_is_rejected_not_treated_as_pool_origin) | test(=mathematical_model::adversarial_properties::model_generated_short_id_collision_is_not_confused_with_full_identity) | test(=mathematical_model::adversarial_properties::model_hostile_universe_preserves_same_raw_distinct_witness_variants) | test(=mathematical_model::develop_refinement::develop_detached_cache_lookup_can_substitute_raw_for_witness_identity) | test(=mathematical_model::evidence_transition_properties::model_acceptance_receipt_requires_chain_key_identity_proof_view_and_dependency_cut) | test(=mathematical_model::evidence_transition_properties::model_final_subject_and_direct_rejection_have_closed_currentness_outcomes) | test(=mathematical_model::evidence_transition_properties::model_missing_source_policy_is_remote_wait_or_trusted_definitive_rejection) | test(=mathematical_model::properties::model_proposal_collision_is_bounded_and_never_aliases_full_transaction_identity) | test(=mathematical_model::properties::model_relay_raw_identity_cannot_alias_a_witness_variant) | test(=mathematical_model::properties::model_verification_cache_identity_is_witness_plus_rules_not_raw_identity)'`

Rust evidence:

- `authority::tests::evidence_refinement::uak_direct_rejection_refines_the_closed_view_and_source_truth_table` (T1, T2, T4, T6, T9, T10, T11)
- `authority::tests::evidence_refinement::uak_final_and_direct_acceptance_refine_current_chain_and_dependency_cuts` (T1, T2, T4, T6, T9, T10, T11)
- `authority::tests::evidence_refinement::uak_final_subject_refines_every_reachable_currentness_outcome` (T1, T2, T4, T6, T9, T10, T11)
- `authority::tests::evidence_refinement::uak_missing_dependency_policy_refines_source_and_parent_ownership` (T1, T2, T4, T6, T9, T10, T11)
- `authority::tests::foundation::uak_short_id_collision_cannot_alias_primary_identity` (T1, T11)
- `authority::tests::resolver::uak_verification_cache_lookup_cannot_substitute_a_nearby_request` (T11)
- `authority::tests::resolver::uak_verification_request_binds_environment_rules_and_witness_cache_key` (T11)
- `authority::tests::validation::uak_final_validation_rejects_a_mixed_authority_snapshot_cut` (T9, T11)
- `authority::tests::validation::uak_same_tip_unproven_location_is_rejected_not_treated_as_pool_origin` (T9, T11)
- `mathematical_model::adversarial_properties::model_generated_short_id_collision_is_not_confused_with_full_identity` (T8, T11)
- `mathematical_model::adversarial_properties::model_hostile_universe_preserves_same_raw_distinct_witness_variants` (T1, T11)
- `mathematical_model::develop_refinement::develop_detached_cache_lookup_can_substitute_raw_for_witness_identity` (T11)
- `mathematical_model::evidence_transition_properties::model_acceptance_receipt_requires_chain_key_identity_proof_view_and_dependency_cut` (T1, T2, T4, T6, T9, T10, T11)
- `mathematical_model::evidence_transition_properties::model_final_subject_and_direct_rejection_have_closed_currentness_outcomes` (T1, T2, T4, T6, T9, T10, T11)
- `mathematical_model::evidence_transition_properties::model_missing_source_policy_is_remote_wait_or_trusted_definitive_rejection` (T1, T2, T4, T6, T9, T10, T11)
- `mathematical_model::properties::model_proposal_collision_is_bounded_and_never_aliases_full_transaction_identity` (T1, T3, T6, T8, T11)
- `mathematical_model::properties::model_relay_raw_identity_cannot_alias_a_witness_variant` (T1, T3, T7, T11)
- `mathematical_model::properties::model_verification_cache_identity_is_witness_plus_rules_not_raw_identity` (T11)

#### `TP-PERF-001` - Bounded work and preserved concurrency

Generated focused command: `cargo nextest run -p ckb-tx-pool --features internal -E 'test(=authority::tests::foundation::uak_checkout_attack_work_is_bounded_by_owner_heads_and_active_slots) | test(=authority::tests::foundation::uak_independent_ready_order_is_invariant_to_worker_completion_permutations) | test(=authority::tests::relay::uak_relay_mailbox_accounting_mismatch_rebuilds_instead_of_saturating) | test(=authority::tests::relay::uak_relay_mailbox_bounds_oversized_parent_detail_without_blocking) | test(=benchmark::debug_tests::profile_span_counters_observe_only_the_active_registered_window) | test(=benchmark::debug_tests::profile_span_counters_reject_an_unregistered_target_span) | test(=block_assembler::candidate_uncles::tests::full_container_keeps_a_hard_global_bound) | test(=component::tests::score_key::ancestor_score_order_is_deterministic_at_extreme_weights) | test(=mathematical_model::adversarial_properties::model_deep_chain_and_fanout_key_work_is_linear_at_configured_scale) | test(=mathematical_model::adversarial_properties::model_every_m2_root_premise_has_a_typed_minimum_counterexample) | test(=mathematical_model::adversarial_properties::model_i2_retained_path_separates_ingress_and_ready_batches_from_per_item_compute) | test(=mathematical_model::adversarial_properties::model_quantitative_equation_rejects_every_configured_bound_overrun) | test(=mathematical_model::adversarial_properties::model_quantitative_equation_separates_linear_work_from_core_wave_applies) | test(=mathematical_model::adversarial_properties::model_ready_batch_bound_is_independent_of_the_current_worker_wave_width) | test(=mathematical_model::adversarial_properties::model_ready_composition_cost_is_consumed_without_a_hand_copied_projection) | test(=mathematical_model::composition_properties::model_ready_footprint_cost_is_linear_in_scanned_owners_and_keys) | test(=mathematical_model::composition_properties::model_remote_source_is_a_footprint_policy_term_not_a_second_authority) | test(=mathematical_model::properties::model_service_ingress_residency_bounds_typed_ordered_senders) | test(=mathematical_model::topology_properties::model_bounded_cache_writer_retains_worker_isolation_with_one_named_cost) | test(=mathematical_model::topology_properties::model_bounded_exchange_is_wave_amortization_not_asymptotic_magic) | test(=mathematical_model::topology_properties::model_complete_topology_selection_rejects_partial_fixes_without_stitching_exceptions) | test(=mathematical_model::topology_properties::model_exchange_cost_names_its_task_channel_and_failure_price) | test(=mathematical_model::topology_properties::model_one_available_wave_falsifies_current_and_self_fused_serial_cut_targets) | test(=service::builder::tests::retained_ingress_batch_preserves_handler_concurrency_at_the_apply_bound)'`

Rust evidence:

- `authority::tests::foundation::uak_checkout_attack_work_is_bounded_by_owner_heads_and_active_slots` (T3, T5, T8)
- `authority::tests::foundation::uak_independent_ready_order_is_invariant_to_worker_completion_permutations` (T5, T8, T10)
- `authority::tests::relay::uak_relay_mailbox_accounting_mismatch_rebuilds_instead_of_saturating` (T7, T8, T10)
- `authority::tests::relay::uak_relay_mailbox_bounds_oversized_parent_detail_without_blocking` (T7, T8, T10)
- `benchmark::debug_tests::profile_span_counters_observe_only_the_active_registered_window` (T8)
- `benchmark::debug_tests::profile_span_counters_reject_an_unregistered_target_span` (T8)
- `block_assembler::candidate_uncles::tests::full_container_keeps_a_hard_global_bound` (T8, T13)
- `component::tests::score_key::ancestor_score_order_is_deterministic_at_extreme_weights` (T8, T13)
- `mathematical_model::adversarial_properties::model_deep_chain_and_fanout_key_work_is_linear_at_configured_scale` (T4, T8, T10)
- `mathematical_model::adversarial_properties::model_every_m2_root_premise_has_a_typed_minimum_counterexample` (T1, T2, T3, T4, T5, T6, T8, T9, T10, T11)
- `mathematical_model::adversarial_properties::model_i2_retained_path_separates_ingress_and_ready_batches_from_per_item_compute` (T3, T5, T6, T8, T10, T16)
- `mathematical_model::adversarial_properties::model_quantitative_equation_rejects_every_configured_bound_overrun` (T3, T8)
- `mathematical_model::adversarial_properties::model_quantitative_equation_separates_linear_work_from_core_wave_applies` (T3, T5, T6, T8, T10)
- `mathematical_model::adversarial_properties::model_ready_batch_bound_is_independent_of_the_current_worker_wave_width` (T5, T6, T8)
- `mathematical_model::adversarial_properties::model_ready_composition_cost_is_consumed_without_a_hand_copied_projection` (T3, T4, T5, T6, T8)
- `mathematical_model::composition_properties::model_ready_footprint_cost_is_linear_in_scanned_owners_and_keys` (T4, T8)
- `mathematical_model::composition_properties::model_remote_source_is_a_footprint_policy_term_not_a_second_authority` (T3, T5, T8)
- `mathematical_model::properties::model_service_ingress_residency_bounds_typed_ordered_senders` (T3, T8, T10)
- `mathematical_model::topology_properties::model_bounded_cache_writer_retains_worker_isolation_with_one_named_cost` (T5, T8, T10, T11)
- `mathematical_model::topology_properties::model_bounded_exchange_is_wave_amortization_not_asymptotic_magic` (T5, T6, T8)
- `mathematical_model::topology_properties::model_complete_topology_selection_rejects_partial_fixes_without_stitching_exceptions` (T1, T2, T3, T5, T6, T8, T10, T12, T14, T15, T16)
- `mathematical_model::topology_properties::model_exchange_cost_names_its_task_channel_and_failure_price` (T2, T3, T5, T8, T10, T15)
- `mathematical_model::topology_properties::model_one_available_wave_falsifies_current_and_self_fused_serial_cut_targets` (T3, T5, T6, T8, T16)
- `service::builder::tests::retained_ingress_batch_preserves_handler_concurrency_at_the_apply_bound` (T8, T16)

Process-level evidence:

- `conditional-reader-fanout`: `test/src/specs/tx_pool/limit.rs::TxPoolLimitAncestorCount` (T3, T4, T6, T8, T10, T13) - A high-fanout dependency shape remains bounded without corrupting accepted causal limits. Paired units: `authority::tests::dependency::uak_popular_dependency_maintenance_has_one_edge_steps_and_key_fairness`, `authority::tests::foundation::uak_capacity_eviction_removes_one_complete_causal_component`, `block_assembler::candidate_uncles::tests::full_container_keeps_a_hard_global_bound`. Command: `make integration CKB_TEST_ARGS='-c 1 TxPoolLimitAncestorCount'`
- `same-lane-relay-continuation`: `test/src/specs/tx_pool/txs_relay_order.rs::TxsRelayOrder` (T2, T4, T5, T8, T10) - Same-lane continuation preserves exact work-capability ownership, dependency order and bounded fair progress. Paired units: `authority::tests::dependency::uak_popular_dependency_maintenance_has_one_edge_steps_and_key_fairness`, `authority::tests::foundation::uak_resolve_to_verify_continuation_changes_no_authority_state`, `authority::tests::foundation::uak_independent_ready_order_is_invariant_to_worker_completion_permutations`. Command: `make integration CKB_TEST_ARGS='-c 1 TxsRelayOrder'`

#### `TP-HANDOFF-001` - Bounded controller and relay handoff conservation

Generated focused command: `cargo nextest run -p ckb-tx-pool --features internal -E 'test(=authority::tests::boundary_refinement::uak_controller_full_and_closed_outcomes_refine_both_relay_sources) | test(=authority::tests::boundary_refinement::uak_controller_proposal_handoff_refines_notification_cuts) | test(=authority::tests::boundary_refinement::uak_controller_remote_handoff_refines_queued_handler_and_response_cuts) | test(=mathematical_model::boundary_trace::tests::boundary_enqueue_failure_releases_remote_and_proposal_handoffs_exactly_once) | test(=mathematical_model::boundary_trace::tests::boundary_trace_composes_controller_authority_effect_and_lifecycle_cuts) | test(=mathematical_model::properties::model_abandoned_response_cannot_veto_a_later_kernel_commit) | test(=mathematical_model::properties::model_callback_reentrancy_rejects_mutation_without_blocking_reads_or_derived_control) | test(=mathematical_model::properties::model_controller_bounds_payload_bytes_as_well_as_request_count) | test(=mathematical_model::properties::model_drain_cannot_drop_an_outstanding_direct_capability) | test(=mathematical_model::properties::model_notification_acknowledgement_is_orthogonal_to_payload_ownership) | test(=mathematical_model::properties::model_relay_batch_abort_releases_only_the_uncommitted_suffix) | test(=mathematical_model::properties::model_relay_handoff_releases_known_state_on_pre_authority_failure) | test(=mathematical_model::properties::model_relay_terminal_rejection_releases_the_exact_authority_handoff) | test(=mathematical_model::topology_properties::model_existing_dispatcher_is_the_zero_topology_cost_ingress_combiner) | test(=mathematical_model::topology_properties::model_typed_ordered_boundary_bounds_external_admin_residency_without_dropping_reorg) | test(=service::builder::tests::retained_ingress_batch_groups_only_the_same_remote_peer) | test(=service::builder::tests::retained_ingress_batch_keeps_a_nonfitting_proposal_message_whole) | test(=service::controller::tests::asynchronous_network_calls_fail_fast_when_the_controller_channel_is_full) | test(=service::controller::tests::closed_administration_lane_releases_the_unique_admission) | test(=service::controller::tests::public_administration_is_linear_across_controller_clones) | test(=service::controller::tests::remote_submit_waits_without_blocking_a_current_thread_runtime) | test(=service::dispatch::tests::remote_partial_commit_acknowledges_only_the_committed_prefix)'`

Rust evidence:

- `authority::tests::boundary_refinement::uak_controller_full_and_closed_outcomes_refine_both_relay_sources` (T1, T6, T10)
- `authority::tests::boundary_refinement::uak_controller_proposal_handoff_refines_notification_cuts` (T1, T6, T10)
- `authority::tests::boundary_refinement::uak_controller_remote_handoff_refines_queued_handler_and_response_cuts` (T1, T6, T10)
- `mathematical_model::boundary_trace::tests::boundary_enqueue_failure_releases_remote_and_proposal_handoffs_exactly_once` (T1, T7, T10)
- `mathematical_model::boundary_trace::tests::boundary_trace_composes_controller_authority_effect_and_lifecycle_cuts` (T1, T2, T6, T7, T10)
- `mathematical_model::properties::model_abandoned_response_cannot_veto_a_later_kernel_commit` (T6, T10)
- `mathematical_model::properties::model_callback_reentrancy_rejects_mutation_without_blocking_reads_or_derived_control` (T6, T10, T12)
- `mathematical_model::properties::model_controller_bounds_payload_bytes_as_well_as_request_count` (T3, T8, T10)
- `mathematical_model::properties::model_drain_cannot_drop_an_outstanding_direct_capability` (T2, T10)
- `mathematical_model::properties::model_notification_acknowledgement_is_orthogonal_to_payload_ownership` (T6, T10)
- `mathematical_model::properties::model_relay_batch_abort_releases_only_the_uncommitted_suffix` (T1, T6, T7, T10)
- `mathematical_model::properties::model_relay_handoff_releases_known_state_on_pre_authority_failure` (T1, T7, T10)
- `mathematical_model::properties::model_relay_terminal_rejection_releases_the_exact_authority_handoff` (T1, T6, T7, T10)
- `mathematical_model::topology_properties::model_existing_dispatcher_is_the_zero_topology_cost_ingress_combiner` (T1, T3, T6, T8, T10, T16)
- `mathematical_model::topology_properties::model_typed_ordered_boundary_bounds_external_admin_residency_without_dropping_reorg` (T3, T8, T9, T10)
- `service::builder::tests::retained_ingress_batch_groups_only_the_same_remote_peer` (T1, T3, T8)
- `service::builder::tests::retained_ingress_batch_keeps_a_nonfitting_proposal_message_whole` (T1, T8, T10)
- `service::controller::tests::asynchronous_network_calls_fail_fast_when_the_controller_channel_is_full` (T8, T10)
- `service::controller::tests::closed_administration_lane_releases_the_unique_admission` (T3, T10)
- `service::controller::tests::public_administration_is_linear_across_controller_clones` (T3, T8, T10)
- `service::controller::tests::remote_submit_waits_without_blocking_a_current_thread_runtime` (T7, T10)
- `service::dispatch::tests::remote_partial_commit_acknowledges_only_the_committed_prefix` (T1, T6, T7, T10)

Cross-crate Rust evidence:

Generated cross-crate command: `cargo nextest run -p ckb-sync -E 'test(=relayer::tests::block_proposal_process::accepted_proposal_consumes_inflight_and_marks_known) | test(=relayer::tests::block_proposal_process::proposal_closed_controller_consumes_request_without_pinning_known) | test(=relayer::tests::transactions_process::remote_closed_controller_releases_known_projection)'`

- `sync/src/relayer/tests/block_proposal_process.rs::relayer::tests::block_proposal_process::accepted_proposal_consumes_inflight_and_marks_known` (T1, T6, T7, T8, T10) - A requested valid Proposal consumes its network request when the response arrives, remains unknown during authority validation, and becomes known only through the committed Accepted effect.
- `sync/src/relayer/tests/block_proposal_process.rs::relayer::tests::block_proposal_process::proposal_closed_controller_consumes_request_without_pinning_known` (T1, T7, T10, T11) - A received Proposal response consumes its bounded request evidence exactly once but remains unknown until committed acceptance, so controller failure cannot suppress supply by another peer or enable response replay.
- `sync/src/relayer/tests/transactions_process.rs::relayer::tests::transactions_process::remote_closed_controller_releases_known_projection` (T1, T7, T10, T11) - A failed Remote controller handoff releases its exact known-filter mark, allowing another peer to reannounce the same raw transaction.

<!-- END GENERATED: TX_POOL_BEHAVIORS -->

## Source and test navigation contract

The generated behavior table is the production navigation index: each row
names exact implementation-owner symbols and exact Rust/process evidence.
Do not maintain a second path table here. Full test discovery is recorded in
`test-inventory.txt`; curated architectural evidence remains intentionally
smaller and lives only in `review-behaviors.json`.

Layout rules are mechanical review guarantees:

- `tests/mod.rs` is the sole root when a module has a `tests/` directory; do
  not add a neighboring `tests.rs`.
- Behavior-bearing files use domain names such as `persistence.rs` or
  `pre_pool_queue.rs`. Helper-only private bridges end in `_test_support.rs`;
  the historical `_seam.rs` name is retired.
- Service-level scenarios and their harness live under `service/tests`, not
  under `component/tests`. Reusable transaction fixtures live in the crate
  `#[cfg(test)]` module `test_support` and do not widen production APIs.
- Production files contain only automatically discovered `cfg(test)` module
  wiring or a named irreducible observation seam. The manifest records only
  the allowed test roots and exceptional seams; it never copies wiring or
  occurrence counts that the checker can discover.
- Renaming or relocating a test regenerates `test-inventory.txt` and the
  generated guide region through maintainer commands. CI remains read-only and
  rejects either artifact when it drifts.

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
  [`test-layout-manifest.json`](../test-layout-manifest.json).
- The security ledger is updated when a risk is accepted rather than fixed;
  accepted residuals state scope, consequence and future trigger.
- Full correctness gates are green. Performance claims wait for controlled A/B
  evidence and cannot be inferred from a noisy quick run.
