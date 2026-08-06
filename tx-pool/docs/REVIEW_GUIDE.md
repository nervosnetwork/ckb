# Tx-Pool Test-Driven Review Guide

This guide is the reviewer entry point for tx-pool changes. It translates the
T1-T13 proof obligations in [`ARCHITECTURE.md`](ARCHITECTURE.md) into stable
`TP-*` behaviors, hostile counterexamples and executable evidence.
The behavior/evidence mapping is generated from
[`review-behaviors.json`](../review-behaviors.json); do not edit the generated
region by hand. File ownership and update commands are defined in
[`VALIDATION.md`](VALIDATION.md). Rows describe required current behavior, not
the history of how it was introduced.

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
5. Apply the repository-root [`AGENTS.md`](../../AGENTS.md) checklist to the whole
   changed architecture, not only the edited function: type/error design,
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

`make integration CKB_TEST_ARGS='-c 1 CellBeingCellDepAndSpentInSameBlockTestGetBlockTemplate TxPoolLimitAncestorCount ReorgRecoversDependentPendingTree RpcTruncate ReorgRecoversDependentChain ReorgRecoversDependentTxs PoolResolveConflictAfterReorg RbfRejectReplaceProposed RbfContainInvalidInput RbfOrphanRecovery RbfBasic RbfReplaceProposedSuccess RbfConcurrency RbfCyclingAttack RbfCellDepsCheck TxPoolOrphanReverse TxsRelayOrder LocalTestSubmissionIsDirect UncleInheritFromForkUncle'`

The complete tx-pool impact universe contains 150 specs. Integration and release CI run the exact inventory through:

`make integration CKB_TEST_ARGS='-c 1 AvoidDuplicatedProposalsWithUncles BlockSyncDuplicatedAndReconnect BlockSyncForks BlockSyncFromOne BlockSyncNonAncestorBestBlocks BlockSyncOrphanBlocks BlockSyncRelayerCollaboration BlockSyncWithUncle BlockTemplates BlockTransactionsRelayParentOfOrphanBlock CellBeingCellDepAndSpentInSameBlockTestGetBlockTemplate CellBeingCellDepAndSpentInSameBlockTestGetBlockTemplateMultiple CellBeingCellDepThenSpentInSameBlockTestSubmitBlock CellBeingSpentThenCellDepInSameBlockTestSubmitBlock ChainFork1 ChainFork2 ChainFork3 ChainFork4 ChainFork5 ChainFork6 ChainFork7 CheckAbsoluteEpochSince CheckCellDeps CheckRelativeEpochSince CheckTypical2In2OutTx CheckVmBExtension CheckVmVersion1 CheckVmVersion2 CompactBlockEmpty CompactBlockEmptyParentUnknown CompactBlockLoseGetBlockTransactions CompactBlockMissingFreshTxs CompactBlockMissingNotFreshTxs CompactBlockMissingWithDropTx CompactBlockPrefilled CompactBlockRelayLessThenSharedBestKnown CompactBlockRelayParentOfOrphanBlock ConflictInGap ConflictInPending ConflictInProposed DAOWithSatoshiCellOccupied DeclaredWrongCycles DeclaredWrongCyclesAndRelayAgain DeclaredWrongCyclesChunk DepentTxInSameBlock DifferentTxsWithSameInputWithOutRBF DuplicatedTransaction FeeOfMaxBlockProposalsLimit FeeOfMultipleMaxBlockProposalsLimit FeeOfTransaction ForkedTransaction ForksContainSameTransactions ForksContainSameUncle GetRawTxPool HandlingDescendantsOfCommitted HandlingDescendantsOfProposed HeaderSyncCycle InboundMinedDuringSync InboundSync InvalidHeaderDep LoadProgramFailedTx LocalTestSubmissionIsDirect LongForks MalformedTx MiningBasic NotifyLargeCyclesTx OrphanTxAccepted OrphanTxRejected OutboundMinedDuringSync OutboundSync PackUnclesIntoEpochStarting PoolPersisted PoolReconcile PoolResolveConflictAfterReorg PoolResurrect ProposalExpireRuleForCommittingAndExpiredAtOneTime ProposalRespondSizelimit ProposeButNotCommit ProposeDuplicated ProposeOutOfOrder ProposeTransactionButParentNot RbfBasic RbfCellDepsCheck RbfChildPayForParent RbfConcurrency RbfContainInvalidCells RbfContainInvalidInput RbfContainNewTx RbfCyclingAttack RbfEnable RbfOnlyForResolveDead RbfOrphanRecovery RbfRejectReplaceProposed RbfReplaceProposedSuccess RbfSameInput RbfSameInputwithLessFee RbfTooManyDescendants RelayInvalidTransaction RelayInvalidTransactionResumable RelayWithWrongTx RemoveConflictFromPending RemoveTx ReorgHandleProposals ReorgRecoversDependentChain ReorgRecoversDependentPendingTree ReorgRecoversDependentTxs RequestUnverifiedBlocks RpcGetBlockTemplate RpcSubmitBlock RpcTruncate SameCellAsInputAndCellDep SendConflictTxToRelay SendConflictTxToRelayRBF SendLargeCyclesTxInBlock SendLargeCyclesTxToRelay SendLowFeeRateTx SendTxChain SendTxChainRevOrder SizeLimit SpendSatoshiCell SubmitConflict SubmitTransactionWhenItsParentInGap SubmitTransactionWhenItsParentInProposed SyncTooNewBlock TooManyUnknownTransactions TransactionHashCollisionDifferentWitnessHashes TransactionRelayBasic TransactionRelayConflict TransactionRelayEmptyPeers TransactionRelayLowFeeRate TransactionRelayTimeout TxPoolEntryStatus TxPoolLimitAncestorCount TxPoolOrphanDoubleSpend TxPoolOrphanNormal TxPoolOrphanPartialInputUnknown TxPoolOrphanReverse TxPoolOrphanUnordered TxsRelayOrder UncleInheritFromForkBlock UncleInheritFromForkUncle ValidSince WithdrawDAO WithdrawDAOWithOverflowCapacity send_defected_binary_do_not_reject_known_bugs send_defected_binary_reject_known_bugs send_multisig_secp_tx_use_dep_group_data_hash send_multisig_secp_tx_use_dep_group_type_hash send_secp_tx_use_dep_group_data_hash send_secp_tx_use_dep_group_type_hash'`

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

### Behavior index

| ID | Implementation owners | Required behavior | Hostile/failure case | Invariants | Reviewer gate | Performance bound |
|---|---|---|---|---|---|---|
| `TP-OWN-001` Single transaction ownership and ABA safety | `tx-pool/src/authority/state.rs`: `enum OwnedTx`, `enum PreAcceptedPhase`, `EntryVersion`<br>`tx-pool/src/authority/work.rs`: `struct SettlementToken`, `enum CheckedOutWork`<br>`tx-pool/src/authority/plan.rs`: `struct TxPoolAuthority`, `struct PreparedApply` | Each raw transaction hash has zero or one OwnedTx in TxPoolAuthority.entries. Compute consumes one move-only work value bound to the exact owner version and Computing phase; chain proof remains bound to its exact view, while other asynchronous receipts carry their own typed generation or source cut. Queues, workers, receipts and effects never own lifecycle state. | Duplicate ingress, source promotion, stale completion, remove/readmit ABA, clear or reorg must not create a second owner, erase the current owner or leave an uncharged payload. | T1, T2, T3, T5, T6, T7, T8, T10, T11 | - Can any payload exist outside entries after an authority guard opens?<br>- Does every stale completion return without semantic mutation?<br>- Can a new phase or owner be assembled without its exact charge and projections? | One owner map and one short authority transition; no compensating owner scan or second lifecycle lock. |
| `TP-COMMIT-001` Read-only Plan and total Apply | `tx-pool/src/authority/plan.rs`: `struct PreparedApply`, `fn plan_final_admission`, `fn apply_membership`, `struct IndependentDelta`<br>`tx-pool/src/authority/plan/settlement.rs`: `fn prepare_verified_compute_settlement`, `fn compile_independent_delta`<br>`tx-pool/src/authority/runtime.rs`: `fn execute_verification`, `fn try_drive_ready`, `fn complete_ready_batch` | All policy, stale, resource, membership and effect decisions finish before a single-use PreparedApply commits owner, projection, charge, clocks and effects. Apply is total; dropped or stale plans are semantically mutation-free. A verified Computing owner may bypass the intermediate Ready location only when a sealed exact-view chain-backed capability and the existing independent membership compiler both prove the direct transition; every miss consumes the same settlement through canonical charged Ready. | Concurrent Ready work, chain-view ABA, RBF or same-input conflict, pool-produced dependency, capacity pressure, effect saturation or allocation failure must not expose partial membership, bypass canonical policy or require rollback. | T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12 | - Does Plan change any authoritative semantic fact?<br>- Can Apply return an ordinary failure or allocate fallibly?<br>- Are independent batches proven commuting, unique and bounded before Apply? | Fallible work and large destruction stay outside Apply. Independent verified transactions may commit in bounded commuting Ready batches; an exact chain-backed singleton may remove the non-semantic Ready round trip without adding a second validation, membership or publication owner. |
| `TP-RBF-001` Atomic deterministic replacement | `tx-pool/src/authority/plan/membership/rbf.rs`: `validate_no_new_unconfirmed_inputs`, `validate_no_victim_dependencies`, `validate_replacement_fee`<br>`tx-pool/src/authority/plan/membership.rs`: `MembershipReject`, `PreparedMembership` | RBF computes the complete bounded victim and descendant closure, all dependency restrictions and both fee gates against one coherent virtual membership before one total replacement Apply. | An under-fee, over-bound, new-unconfirmed-input, victim-dependent, self-evicting or concurrent candidate must leave every existing owner and aggregate unchanged. | T1, T2, T3, T4, T6, T7, T8, T9, T10, T11, T12, T13 | - Are all victims and descendants included exactly once before fees are evaluated?<br>- Can any victim move before the winner and complete history disposition are known?<br>- Is every positive chain-input premise explicit and exact? | Conflict work follows bounded indexes and cohorts; no speculative removal, undo engine or unbounded full-pool scan. |
| `TP-DEP-001` Exact dependency and level-triggered progress | `tx-pool/src/authority/dependency.rs`: `struct DependencyFrontier`, `struct DependencyMaintenanceTicket`, `enum DependencyMaintenanceStep`<br>`tx-pool/src/authority/resolver.rs`: `collect_missing_against_cut`, `resolve_candidate` | Canonical input, cell-dep, header and expanded dep-group evidence drives one DependencyFrontier. Missing observations carry an exact cut; availability and definitive loss advance the same level in the owner-changing Apply. | Parent death, repeated availability, source promotion, high fanout, late dep-group discovery or coalesced wakes must not strand a child, accept stale evidence or spin indefinitely. | T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13 | - Does every producer loss reach every surviving indexed consumer exactly once?<br>- Does a waiter subscribe before checking the authoritative level?<br>- Are fanout and maintenance work sliced by explicit bounds? | Indexed key-scoped maintenance replaces polling and population scans; each maintenance step has a fixed work bound. |
| `TP-CACHE-001` Bounded replacement history and recovery | `tx-pool/src/authority/state.rs`: `struct ReplacementHistoryEntry`, `ReplacementHistoryCharge`<br>`tx-pool/src/authority/plan.rs`: `fn plan_replacement_history_admission`, `ReplacementHistoryLimit`<br>`tx-pool/src/authority/plan/chain_transition.rs`: `fn chain_dependency_events`, `chain_available`<br>`tx-pool/src/authority/plan/membership.rs`: `fn spender_after` | Only an actually Accepted RBF victim can become inert charged ReplacementHistory. It observes exact final blockers, and chain-layer availability becomes a wake only after the same Apply's final Accepted overlay has no spender. History has no scheduler/source/peer/deadline, remains private to live RPC/template/persistence, is the sole source of the legacy conflicted hash projection, and re-enters full validation on recovery. Failed candidates terminalize into per-hash reject evidence without retained ownership. | A policy-rejected loser, remote candidate, same-Apply victim wake, partial blocker release, unrelated chain change or saturated history budget must not create executable ghost work, hidden residency, false wake or partial retained history. | T1, T2, T3, T4, T6, T7, T8, T9, T10, T11, T12, T13 | - Is the constructor reachable only from successful Accepted-victim displacement?<br>- Does wake require a newer level for every retained blocker?<br>- Does saturation drop the complete optional set while preserving the winner? | History has independent count/byte/edge bounds and no active-work or scheduler cost. |
| `TP-BUDGET-001` Continuous hostile-resource accounting | `tx-pool/src/authority/resources.rs`: `struct ResourceLedger`, `struct ResourceLimits`, `enum ResourceError`, `struct ComputeLimits`<br>`tx-pool/src/authority/runtime.rs`: `struct ComputeGate`, `AuthorityComputeExecutionPermit` | Every owner continuously carries exact entry, byte, edge, active-work and compute-reservation charges, with accepted, remote, per-peer and replacement-history sublimits validated by checked construction. | Payload growth, expanded dependency fanout, promotion, active work, ghost charge, arithmetic overflow or peer churn must fail before mutation and cannot escape or double charge across phases. | T1, T3, T4, T6, T7, T8, T10, T11 | - Does owner existence imply exactly one ledger row and vice versa?<br>- Is attacker-shaped growth reserved before it is retained?<br>- Are all sums and limit hierarchies checked? | Accounting is sparse and transition-local; no global recount on ordinary ingress or settlement. |
| `TP-WORKER-001` Capability-owned workers and progress | `tx-pool/src/authority/plan.rs`: `struct AuthorityWakeTransition`, `struct ComputeWakeTransition`<br>`tx-pool/src/authority/worker.rs`: `struct ComputeWorker`, `run_ready_driver`, `run_maintenance_driver`<br>`tx-pool/src/authority/scheduler.rs`: `struct FairFrontier`, `struct CheckoutTicket`<br>`tx-pool/src/authority/runtime.rs`: `struct AuthoritySignals`, `fn publish_post_commit`, `fn publish_wake`, `fn try_checkout` | Workers hold one move-only compute capability, perform expensive work without the store guard, and settle or cancel it exactly once. The exhaustive Apply compiler derives a move-only post-commit wake transition from both scheduler heads and active-work availability; only after the guard opens and retirement completes does one central router prompt resolver, verifier capability, Ready, maintenance, effect, capacity and template consumers. A stable queued head is republished when a charged active-work slot is released, because global, source or peer limits may previously have made it ineligible. Hints carry no authority and every wait rechecks its exact runnable condition. Resolve and Verify use cancellation-aware per-owner round robin; Ready uses the documented strict source/economic order in bounded batches and does not claim per-entry fairness. | A wrong capability consuming a coalesced hint, heterogeneous effect-capacity waiters, missing post-commit publication, saturated peer, stale queue head, allocation retry, cancellation or completion reordering must not lose a work capability, abort unrelated work, spin or starve an eligible Resolve/Verify owner. Strict Ready priority must remain deterministic, batch-bounded and subject to Remote expiry. | T2, T4, T5, T8, T10 | - Does every wait name an independent releaser?<br>- Can a worker exit while retaining the only active capability?<br>- Is queue fairness decided only from committed scheduler state, and is strict Ready priority kept separate?<br>- Does every runtime mutation consume one top-level post-commit receipt with no escaping control flow?<br>- Can any role consume the only hint for work that role cannot guarantee to service?<br>- Does runnable publication cover resource-eligibility changes even when the scheduler head identity is stable? | Resolve/verify remains parallel. Allocation-free committed-head and active-work projections route wake-one batons to one capability-compatible waiter set. Releasing one active-work slot may conservatively republish each retained compatible head, bounded by completed compute work; heterogeneous effect-capacity and changed template sources alone retain bounded broadcast. No signal is a second scheduler or membership authority. |
| `TP-ADMIN-001` Cause-complete administration and peer revocation | `tx-pool/src/authority/plan.rs`: `enum AdminPlan`, `struct OwnerRemovalBatch`, `fn plan_administrative_removal`, `fn plan_peer_revocation`, `fn plan_local_removal`<br>`tx-pool/src/authority/ban.rs`: `struct PeerBanRegistry`, `struct PeerBanLease` | Clear, expiry, local removal and peer revocation use one cause-complete owner-removal compiler. Peer ban removes only not-yet-Accepted ingress owners; every delayed Remote message rejected by a retained peer fence commits its own exact relay release so another peer may refetch it. The fence registry has a hard count bound; saturation retires the oldest fence and sends any later delayed submission through complete bounded validation instead of blocking all Remote ingress. | Ban during compute, promoted remote ingress, delayed controller delivery after the revocation reset, accepted descendants, repeated ban, unbounded distinct-session churn, expiry, clear or local removal must not leave unbounded state, active work, dependency edges, charge or known-filter state behind; Accepted owners must not be removed by peer ban. | T1, T2, T3, T4, T6, T7, T8, T9, T10, T11, T12 | - Does each cause map all owner variants exhaustively?<br>- Are Accepted and not-yet-Accepted ban semantics separated by type?<br>- Does every remote removal publish refetch cleanliness exactly once?<br>- Is peer-fence saturation hard-bounded and limited to the documented oldest-session validation fallback? | Removal follows exact indexed cohorts and bounded descendant closure; the peer fence is fixed-size with amortized expiry/oldest eviction and no active-work drain, full scan or global Remote stop. |
| `TP-EFFECT-001` Atomic bounded effects and publication | `tx-pool/src/authority/effect.rs`: `enum CommittedEffect`, `struct EffectLog`, `enum EffectPolicy`, `struct EffectLimits`<br>`tx-pool/src/authority/publisher.rs`: `run_claimed_authority_effect_publisher`, `struct AuthorityEffectEndpoints` | Required callback, relay, reject, peer and parent-request outcomes are bounded immutable effects committed by the same Apply. One claimed publisher consumes exact leases after the store guard opens and settles progress without rereading ownership. | Journal saturation, relay disconnect, slow callback, generation reset, publisher cancellation or endpoint retry must not roll back state, replay completed endpoints, lose the effect lease or block authority progress indefinitely. | T1, T2, T3, T4, T6, T7, T8, T10 | - Does every public terminal outcome carry its effect in the same Plan?<br>- Can only rebuildable detail collapse to GenerationReset?<br>- Are region and indivisible-batch capacities valid at startup? | Publication is outside the authority guard; Remote, Trusted and Critical capacity prevent hostile head blocking. |
| `TP-REORG-001` Reliable atomic chain reconciliation | `tx-pool/src/authority/chain_boundary.rs`: `struct ChainUpdateRequest`, `enum ChainPackaging`, `enum ChainBoundaryError`<br>`tx-pool/src/authority/plan/chain_transition.rs`: `fn plan_chain_transition`, `fn plan_chain_generation_replacement`<br>`tx-pool/src/authority/service.rs`: `fn run_ordered_chain_control_driver`<br>`tx-pool/src/service/message.rs`: `enum ChainControl`<br>`chain/src/verify.rs`: `install_chain_tip_transition` | One readiness-independent capacity-one control boundary pairs each installed snapshot with its exact fork delta and orders later generation clears after that reconciliation. One UAK Apply reconciles status, membership, recovery, dependencies, resources, effects and chain view. | Blank-fork reorg, Gap outside the new window, detached dependency tree, short-ID collision, over-bound recovery, startup readiness false or truncate followed by clear must not lose a tip delta, let delayed recovery overtake clear, expose a partial owner generation or strand re-proposal. | T1, T3, T4, T5, T6, T8, T9, T10, T12, T13 | - Does every best-tip installation call the sole reliable boundary?<br>- Can ClearPool or ClearPipeline overtake an already-published chain transition?<br>- Can any derived retry replay authoritative chain mutation?<br>- Are recovered owners parent-first and ordinary validation inputs? | Only chain reconciliation and rare generation clears share the ordered lane; admission remains concurrent, packaging and sorting occur outside the guard, and derived template work remains separate. |
| `TP-PERSIST-001` Coherent bounded persistence | `tx-pool/src/authority/read.rs`: `fn capture_persistence`, `struct PersistenceReadReceipt`, `struct ParentFirstPersistence`<br>`tx-pool/src/authority/service.rs`: `fn save_pool`, `fn replay_persisted`<br>`tx-pool/src/persisted.rs`: `struct PersistenceWriter`, `struct PersistenceSnapshot` | Persistence captures Accepted and Recovery-source owners from one authority read cut, releases the guard, orders parent-first, writes one bounded atomic snapshot and revalidates every replayed transaction. Startup derives one checked read ceiling and enforces it both before allocation and while the file is read. | Save racing reorg/clear, recovery in any phase, malformed, growing or oversized file, configuration arithmetic overflow, duplicate partitions, writer failure or legacy v1 input must not persist an ownership splice, allocate unbounded data or trust stale verification evidence. | T4, T8, T9, T10, T11, T12 | - Are ReplacementHistory, Remote and Proposal owners excluded?<br>- Does sorting and file I/O occur after the authority guard opens?<br>- Are configured and observed file bounds checked before allocation, during reading and before fully validated replay? | Persistence clone/sort/I/O is off admission paths and serialized by one writer capability. |
| `TP-QUERY-001` Coherent public projections | `tx-pool/src/authority/read.rs`: `struct AuthorityReadView`, `enum AuthorityReadState`, `fn rpc_status_for_accepted`<br>`tx-pool/src/authority/query.rs`: `enum AuthorityTransactionLookup`, `struct PersistenceReceipt` | RPC, compact-block, live-cell, pool detail, fee and persistence queries capture one coherent authority cut and finish fallible or external work after releasing the guard. Compatibility mapping never drives internal state. | Concurrent clear/reorg/admission, short-ID collision, ReplacementHistory lookup, storage delay or allocation failure must not splice generations, fabricate proof/status or hold the authority lock across I/O. | T1, T3, T6, T7, T8, T9, T10, T12 | - Does each receipt own every value needed after the guard opens?<br>- Is ReplacementHistory hidden from the live RPC state?<br>- Can public Pending ever be reused as an internal phase decision? | Shared read cuts stay short; sorting, lookup and serialization are outside the store guard. |
| `TP-DEFECT-001` Rust-native ordinary failure boundary | `tx-pool/src/authority/service.rs`: `enum AuthorityServiceError`, `enum AuthorityIntegrityFault`, `fn settle_operation_error`, `enum AuthorityProjectionFault`<br>`tx-pool/src/authority/topology.rs`: `enum AuthorityGenerationFault`, `enum AuthorityDerivedTaskFailure`, `enum AuthorityShutdownStatus` | Legal, hostile, stale, duplicate, resource, cancellation and external or rebuildable-derived failures are typed local outcomes. Only a proven authority contradiction or loss of a sole linear capability can forbid persistence; panic-and-catch and broad fail-stop are not control flow. | Malformed transactions, allocation pressure, relay/cache/template failure, candidate-uncle source exhaustion, recent-reject encoding, task exit or endpoint timeout must not be mislabeled as peer rejection, silently accepted or escalated to service stop without a structural proof. | T1, T2, T3, T6, T7, T8, T9, T10, T11, T13 | - Can valid or hostile input construct any AuthorityFault or generation invalidity?<br>- Are derived failures isolated from transaction authority?<br>- Does production use panic-like operations as validation or retry logic? | Typed branches add no repair scan, restart loop or catch-unwind boundary to hot paths. |
| `TP-POOL-001` Atomic accepted membership graph | `tx-pool/src/authority/plan/membership.rs`: `struct MembershipProjection`, `struct PreparedMembership`, `struct ProjectionDelta`<br>`tx-pool/src/authority/plan/membership/eviction.rs`: `fn apply_removals`, `fn apply_candidate` | Accepted ownership, spender relation, causal parents, aggregates, status counts and eviction order are one derived MembershipProjection changed atomically with owners and resources. | Diamond/fan-in graph, late parent, conditional reader/spender, capacity eviction, status change or RBF descendant removal must not leave a surviving invalid consumer, stale aggregate or partial component. | T1, T3, T4, T5, T6, T8, T9, T10, T11, T12, T13 | - Does one canonical graph delta update every affected aggregate once?<br>- Are inputs and cell-dep reader relations intentionally distinct?<br>- Does capacity remove a complete valid component and never a candidate ancestor? | Sparse bounded graph deltas replace full-pool rebuilds; deterministic order uses checked integer comparison. |
| `TP-TEMPLATE-001` Concurrent versioned template convergence | `tx-pool/src/authority/template.rs`: `struct TemplateConvergence`, `struct AuthorityTemplateReadReceipt`, `enum TemplatePublication`<br>`tx-pool/src/authority/template_driver.rs`: `struct AuthorityBlockAssembler`, `fn run_replacement_lane`, `fn run_component_lane`<br>`tx-pool/src/authority/packing.rs`: `struct TemplatePackingLimits` | One ordered full/reset lane and optimistic proposal, transaction and uncle lanes build from immutable receipts and publish only against current chain/source/template versions. The block assembler remains derived and rebuildable. | Recovered Gap tree, detached uncle proposal overlap, stale reset/full, conditional cycle, CPFP fan-in, optional byte pressure or rebuild failure must not publish invalid order, censor re-proposal, overwrite newer output or spin. | T1, T4, T5, T6, T8, T9, T10, T11, T12, T13 | - Are full/reset serialized without serializing partial/uncle construction?<br>- Does every lane publish only an exact current receipt?<br>- Are optional proposals/uncles packed under one byte budget using only published conflicts? | Builds run outside authority/publication guards; O(1) source probes skip irrelevant population capture. |
| `TP-IDENTITY-001` Exact transaction and evidence identity | `tx-pool/src/authority/state.rs`: `struct TxIdentity`, `struct RawTxHash`, `struct WitnessTxHash`, `struct ProposalId`<br>`tx-pool/src/authority/chain.rs`: `struct CellLocationReceipt`, `struct VerificationContextReceipt`, `struct TxPoolChainBackedFinalAdmissionWork`, `struct TxPoolComputeAdmissionReceipt`<br>`verification/src/cache.rs`: `struct TxVerificationCacheKey`, `enum ScriptVerificationRules`<br>`tx-pool/src/authority/validation.rs`: `struct FinalAdmissionValidation`, `fn validate_membership` | Ownership uses raw hash, script-cache reuse uses inline witness hash plus script rules, proposal short ID is collision-aware protocol indexing, and chain evidence is bound to an exact snapshot/view and per-cell provenance. The tx-pool-only direct-finalization capability requires the verification context's exact ChainViewId and positive chain provenance for every input and expanded cell dependency; block validation cannot construct or consume it. | Witness variant, short-ID collision, nearby cache request, hardfork rule change, mixed snapshot, same-tip unproven cell or chain-view ABA must not reuse another transaction or context's proof. | T1, T2, T9, T11 | - Can a cache caller omit either witness identity or script-rule generation?<br>- Can a short ID decide ownership or duplicate identity?<br>- Is every positive evidence reuse per-input and tx-pool-only? | The cache key is a copy-cheap fixed array plus small enum; exact evidence reuse avoids redundant same-tip chain reads without a new cache. |
| `TP-PERF-001` Bounded work and preserved concurrency | `tx-pool/src/authority/runtime.rs`: `struct AuthorityStoreLock`, `fn execute_resolution`, `fn execute_verification`, `fn try_drive_ready`<br>`tx-pool/src/authority/publisher.rs`: `fn publish_checked_out_effect_batch`<br>`tx-pool/src/authority/scheduler.rs`: `struct FairFrontier`, `struct ReadyKey`<br>`tx-pool/src/authority/relay.rs`: `struct AuthorityRelaySink`, `struct AuthorityRelayReceiver`, `fn production_authority_relay_mailbox`<br>`tx-pool/src/block_assembler/candidate_uncles.rs`: `struct CandidateUncles` | Peer-controlled count, bytes, edges, fanout, closure, probes, channels and candidate sets are bounded while independent resolution/verification and optimistic template work remain concurrent and deterministic. | Idle-peer cardinality, saturated owner, long conditional graph, mailbox overflow, full candidate-uncle set, extreme fee weights or controller pressure must not cause unbounded scan, overflow, global serialization or nondeterministic order. | T2, T3, T4, T5, T6, T7, T8, T10, T13 | - Is every hostile loop bounded by a named constant or charged set?<br>- Does an optimization preserve the one authority and exact validation?<br>- Has every statically inferable complexity issue closed before profiling? | No optimization claim is accepted without operation-count evidence and fixed-binary A/B after correctness gates. |

### Executable evidence

#### `TP-OWN-001` - Single transaction ownership and ABA safety

Generated focused command: `cargo nextest run -p ckb-tx-pool --features internal -E 'test(=authority::tests::foundation::uak_active_trusted_witness_replacement_atomically_stales_obsolete_work) | test(=authority::tests::foundation::uak_all_four_preaccepted_phases_are_closed_variants) | test(=authority::tests::foundation::uak_direct_local_admission_moves_from_absent_to_accepted_in_one_apply) | test(=authority::tests::foundation::uak_duplicate_and_promotion_never_create_second_owner) | test(=authority::tests::foundation::uak_stale_compute_version_is_mutation_free_across_aba)'`

Rust evidence:

- `authority::tests::foundation::uak_active_trusted_witness_replacement_atomically_stales_obsolete_work` (T1, T2, T11)
- `authority::tests::foundation::uak_all_four_preaccepted_phases_are_closed_variants` (T1, T5)
- `authority::tests::foundation::uak_direct_local_admission_moves_from_absent_to_accepted_in_one_apply` (T1, T3, T6, T7)
- `authority::tests::foundation::uak_duplicate_and_promotion_never_create_second_owner` (T1, T2, T3)
- `authority::tests::foundation::uak_stale_compute_version_is_mutation_free_across_aba` (T1, T2, T6)

Cross-crate Rust evidence:

Generated cross-crate command: `cargo nextest run -p ckb-rpc -E 'test(=tests::examples::test_rpc_examples)'`

- `rpc/src/tests/examples.rs::tests::examples::test_rpc_examples` (T1, T6, T10) - The local-test RPC starts from an absent transaction and observes the final direct admission result instead of relying on the retired verify-queue acknowledgement gap.

Process-level evidence:

- `local-test-rpc-direct`: `test/src/specs/tx_pool/local_test_submission.rs::LocalTestSubmissionIsDirect` (T1, T2, T3, T6, T7, T8, T11) - Local admission remains direct and atomic while TestAccept remains read-only under the same policy. Paired units: `authority::tests::foundation::uak_direct_local_admission_moves_from_absent_to_accepted_in_one_apply`, `authority::tests::chain::uak_final_admission_receipt_is_stale_after_chain_view_aba`, `authority::tests::resolver::uak_duplicate_inputs_are_a_malformed_outcome_not_an_authority_fault`. Command: `make integration CKB_TEST_ARGS='-c 1 LocalTestSubmissionIsDirect'`

#### `TP-COMMIT-001` - Read-only Plan and total Apply

Generated focused command: `cargo nextest run -p ckb-tx-pool --features internal -E 'test(=authority::runtime::tests::runtime_chain_backed_verification_finalizes_without_ready_round_trip) | test(=authority::runtime::tests::runtime_direct_finalization_falls_back_for_effect_pressure_and_existing_ready) | test(=authority::tests::chain::uak_final_admission_receipt_is_stale_after_chain_view_aba) | test(=authority::tests::foundation::uak_dropped_prepared_apply_is_semantically_mutation_free) | test(=authority::tests::foundation::uak_independent_run_matches_every_canonical_single_prefix) | test(=authority::tests::foundation::uak_terminal_outcome_and_effect_commit_together)'`

Rust evidence:

- `authority::runtime::tests::runtime_chain_backed_verification_finalizes_without_ready_round_trip` (T1, T2, T3, T6, T7)
- `authority::runtime::tests::runtime_direct_finalization_falls_back_for_effect_pressure_and_existing_ready` (T1, T3, T5, T6, T7, T10)
- `authority::tests::chain::uak_final_admission_receipt_is_stale_after_chain_view_aba` (T2, T6, T9, T11)
- `authority::tests::foundation::uak_dropped_prepared_apply_is_semantically_mutation_free` (T6)
- `authority::tests::foundation::uak_independent_run_matches_every_canonical_single_prefix` (T3, T4, T5, T6)
- `authority::tests::foundation::uak_terminal_outcome_and_effect_commit_together` (T6, T7)

Cross-crate Rust evidence:

Generated cross-crate command: `cargo nextest run -p ckb-rpc -E 'test(=tests::examples::test_rpc_examples)'`

- `rpc/src/tests/examples.rs::tests::examples::test_rpc_examples` (T1, T6, T10) - The local-test RPC starts from an absent transaction and observes the final direct admission result instead of relying on the retired verify-queue acknowledgement gap.

Process-level evidence:

- `failed-rbf-terminal`: `test/src/specs/tx_pool/replace.rs::RbfContainInvalidInput` (T1, T3, T6, T7, T8, T12) - A policy-rejected RBF candidate publishes per-hash rejection while preserving every accepted owner and creating no uncharged conflict-history residency. Paired units: `authority::tests::foundation::uak_terminal_outcome_and_effect_commit_together`, `authority::tests::foundation::uak_failed_rbf_fee_disposition_preserves_victims_and_terminalizes_candidate`, `authority::tests::foundation::uak_independent_rbf_churn_never_exceeds_replacement_history_budget`, `authority::tests::query::uak_owned_transaction_query_hides_replacement_history_and_reports_minimum_fee`. Command: `make integration CKB_TEST_ARGS='-c 1 RbfContainInvalidInput'`
- `local-test-rpc-direct`: `test/src/specs/tx_pool/local_test_submission.rs::LocalTestSubmissionIsDirect` (T1, T2, T3, T6, T7, T8, T11) - Local admission remains direct and atomic while TestAccept remains read-only under the same policy. Paired units: `authority::tests::foundation::uak_direct_local_admission_moves_from_absent_to_accepted_in_one_apply`, `authority::tests::chain::uak_final_admission_receipt_is_stale_after_chain_view_aba`, `authority::tests::resolver::uak_duplicate_inputs_are_a_malformed_outcome_not_an_authority_fault`. Command: `make integration CKB_TEST_ARGS='-c 1 LocalTestSubmissionIsDirect'`

#### `TP-RBF-001` - Atomic deterministic replacement

Generated focused command: `cargo nextest run -p ckb-tx-pool --features internal -E 'test(=authority::tests::foundation::uak_failed_rbf_fee_disposition_preserves_victims_and_terminalizes_candidate) | test(=authority::tests::foundation::uak_rbf_accepts_new_input_only_with_positive_chain_evidence) | test(=authority::tests::foundation::uak_rbf_component_bound_stops_before_any_authority_mutation) | test(=authority::tests::foundation::uak_rbf_dependency_on_any_victim_is_mutation_free) | test(=authority::tests::foundation::uak_rbf_replaces_the_complete_descendant_closure_atomically)'`

Rust evidence:

- `authority::tests::foundation::uak_failed_rbf_fee_disposition_preserves_victims_and_terminalizes_candidate` (T1, T3, T6, T7)
- `authority::tests::foundation::uak_rbf_accepts_new_input_only_with_positive_chain_evidence` (T6, T11)
- `authority::tests::foundation::uak_rbf_component_bound_stops_before_any_authority_mutation` (T6, T8)
- `authority::tests::foundation::uak_rbf_dependency_on_any_victim_is_mutation_free` (T4, T6, T11)
- `authority::tests::foundation::uak_rbf_replaces_the_complete_descendant_closure_atomically` (T1, T3, T4, T6)

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

Generated focused command: `cargo nextest run -p ckb-tx-pool --features internal -E 'test(=authority::tests::dependency::uak_dependency_loss_is_exact_key_scoped) | test(=authority::tests::dependency::uak_direct_parent_acceptance_publishes_output_availability_atomically) | test(=authority::tests::dependency::uak_parent_terminalization_cannot_strand_trusted_child) | test(=authority::tests::dependency::uak_popular_dependency_maintenance_has_one_edge_steps_and_key_fairness) | test(=authority::tests::foundation::uak_coupled_membership_requires_exact_positive_input_evidence)'`

Rust evidence:

- `authority::tests::dependency::uak_dependency_loss_is_exact_key_scoped` (T4, T10)
- `authority::tests::dependency::uak_direct_parent_acceptance_publishes_output_availability_atomically` (T4, T6, T10)
- `authority::tests::dependency::uak_parent_terminalization_cannot_strand_trusted_child` (T4, T10)
- `authority::tests::dependency::uak_popular_dependency_maintenance_has_one_edge_steps_and_key_fairness` (T4, T8, T10)
- `authority::tests::foundation::uak_coupled_membership_requires_exact_positive_input_evidence` (T4, T6, T11)

Process-level evidence:

- `accepted-descendants-after-parent-reorg`: `test/src/specs/tx_pool/pool_reconcile.rs::PoolResolveConflictAfterReorg` (T1, T3, T4, T6, T9, T10) - A detached accepted parent and surviving descendant closure reconcile as one valid owner generation. Paired units: `authority::tests::dependency::uak_parent_terminalization_cannot_strand_trusted_child`, `authority::tests::chain::uak_detached_parent_and_accepted_child_recover_parent_first`, `authority::tests::foundation::uak_capacity_eviction_removes_one_complete_causal_component`. Command: `make integration CKB_TEST_ARGS='-c 1 PoolResolveConflictAfterReorg'`
- `cell-dep-arrival-order`: `test/src/specs/tx_pool/dead_cell_deps.rs::CellBeingCellDepAndSpentInSameBlockTestGetBlockTemplate` (T4, T6, T11, T12, T13) - A selected cell-dep reader remains ordered before the spender regardless of arrival order. Paired units: `authority::tests::foundation::uak_coupled_membership_requires_exact_positive_input_evidence`, `authority::tests::template::uak_template_read_receipt_shares_order_and_complete_resolved_payload`. Command: `make integration CKB_TEST_ARGS='-c 1 CellBeingCellDepAndSpentInSameBlockTestGetBlockTemplate'`
- `conditional-reader-fanout`: `test/src/specs/tx_pool/limit.rs::TxPoolLimitAncestorCount` (T3, T4, T6, T8, T10, T13) - A high-fanout dependency shape remains bounded without corrupting accepted causal limits. Paired units: `authority::tests::dependency::uak_popular_dependency_maintenance_has_one_edge_steps_and_key_fairness`, `authority::tests::foundation::uak_capacity_eviction_removes_one_complete_causal_component`, `block_assembler::candidate_uncles::tests::full_container_keeps_a_hard_global_bound`. Command: `make integration CKB_TEST_ARGS='-c 1 TxPoolLimitAncestorCount'`
- `dependent-reorg-chain`: `test/src/specs/tx_pool/reorg_recovers_dependent.rs::ReorgRecoversDependentChain` (T1, T4, T6, T9, T10) - Detached dependent transactions recover parent-first without a lost dependency wake. Paired units: `authority::tests::dependency::uak_parent_terminalization_cannot_strand_trusted_child`, `authority::tests::chain::uak_detached_parent_and_accepted_child_recover_parent_first`. Command: `make integration CKB_TEST_ARGS='-c 1 ReorgRecoversDependentChain'`
- `dependent-reorg-transactions`: `test/src/specs/tx_pool/reorg_recovers_dependent.rs::ReorgRecoversDependentTxs` (T1, T4, T6, T9, T10) - Ordinary detached transaction recovery preserves dependency order and exact chain generation. Paired units: `authority::tests::dependency::uak_parent_terminalization_cannot_strand_trusted_child`, `authority::tests::chain::uak_detached_parent_and_accepted_child_recover_parent_first`. Command: `make integration CKB_TEST_ARGS='-c 1 ReorgRecoversDependentTxs'`
- `multi-parent-orphan-frontier`: `test/src/specs/tx_pool/orphan_tx.rs::TxPoolOrphanReverse` (T4, T6, T7, T10) - A complete multi-parent missing frontier and its relay request commit in one bounded transition. Paired units: `authority::tests::dependency::uak_direct_parent_acceptance_publishes_output_availability_atomically`, `authority::tests::effect::uak_remote_missing_wait_and_parent_request_share_one_backpressured_apply`. Command: `make integration CKB_TEST_ARGS='-c 1 TxPoolOrphanReverse'`
- `same-lane-relay-continuation`: `test/src/specs/tx_pool/txs_relay_order.rs::TxsRelayOrder` (T2, T4, T5, T8, T10) - Same-lane continuation preserves exact work-capability ownership, dependency order and bounded fair progress. Paired units: `authority::tests::dependency::uak_popular_dependency_maintenance_has_one_edge_steps_and_key_fairness`, `authority::tests::foundation::uak_resolve_to_verify_continuation_changes_no_authority_state`, `authority::tests::foundation::uak_independent_ready_order_is_invariant_to_worker_completion_permutations`. Command: `make integration CKB_TEST_ARGS='-c 1 TxsRelayOrder'`

#### `TP-CACHE-001` - Bounded replacement history and recovery

Generated focused command: `cargo nextest run -p ckb-tx-pool --features internal -E 'test(=authority::tests::chain::uak_chain_output_availability_respects_a_surviving_pool_spender) | test(=authority::tests::chain::uak_replacement_history_survives_winner_commit_and_wakes_after_reorg) | test(=authority::tests::foundation::uak_independent_rbf_churn_never_exceeds_replacement_history_budget) | test(=authority::tests::foundation::uak_replacement_history_requires_trusted_proposal_to_promote) | test(=authority::tests::foundation::uak_replacement_history_waits_for_every_observed_blocker) | test(=authority::tests::foundation::uak_replacement_history_wakes_only_on_newer_projected_availability)'`

Rust evidence:

- `authority::tests::chain::uak_chain_output_availability_respects_a_surviving_pool_spender` (T1, T4, T6, T9, T10)
- `authority::tests::chain::uak_replacement_history_survives_winner_commit_and_wakes_after_reorg` (T1, T4, T6, T9, T10)
- `authority::tests::foundation::uak_independent_rbf_churn_never_exceeds_replacement_history_budget` (T3, T8)
- `authority::tests::foundation::uak_replacement_history_requires_trusted_proposal_to_promote` (T1, T11)
- `authority::tests::foundation::uak_replacement_history_waits_for_every_observed_blocker` (T4, T10)
- `authority::tests::foundation::uak_replacement_history_wakes_only_on_newer_projected_availability` (T2, T4, T10)

Process-level evidence:

- `failed-rbf-terminal`: `test/src/specs/tx_pool/replace.rs::RbfContainInvalidInput` (T1, T3, T6, T7, T8, T12) - A policy-rejected RBF candidate publishes per-hash rejection while preserving every accepted owner and creating no uncharged conflict-history residency. Paired units: `authority::tests::foundation::uak_terminal_outcome_and_effect_commit_together`, `authority::tests::foundation::uak_failed_rbf_fee_disposition_preserves_victims_and_terminalizes_candidate`, `authority::tests::foundation::uak_independent_rbf_churn_never_exceeds_replacement_history_budget`, `authority::tests::query::uak_owned_transaction_query_hides_replacement_history_and_reports_minimum_fee`. Command: `make integration CKB_TEST_ARGS='-c 1 RbfContainInvalidInput'`
- `rbf-basic`: `test/src/specs/tx_pool/replace.rs::RbfBasic` (T1, T3, T4, T6, T7, T10) - A valid replacement atomically installs the winner and bounded recovery disposition. Paired units: `authority::tests::foundation::uak_replacement_history_wakes_only_on_newer_projected_availability`, `authority::tests::foundation::uak_rbf_replaces_the_complete_descendant_closure_atomically`, `authority::tests::foundation::uak_failed_rbf_fee_disposition_preserves_victims_and_terminalizes_candidate`. Command: `make integration CKB_TEST_ARGS='-c 1 RbfBasic'`
- `rbf-concurrency`: `test/src/specs/tx_pool/replace.rs::RbfConcurrency` (T1, T2, T6, T10) - Concurrent replacements preserve one highest-fee winner; direct policy losers terminalize, while only contenders actually displaced from Accepted enter charged replacement history. Paired units: `authority::tests::foundation::uak_replacement_history_wakes_only_on_newer_projected_availability`, `authority::tests::foundation::uak_failed_rbf_fee_disposition_preserves_victims_and_terminalizes_candidate`. Command: `make integration CKB_TEST_ARGS='-c 1 RbfConcurrency'`
- `rbf-conflict-recovery`: `test/src/specs/tx_pool/orphan_tx_recovery.rs::RbfOrphanRecovery` (T1, T3, T4, T6, T7, T9, T10) - Replacement history recovers only after a newer exact blocker release and re-enters validation. Paired units: `authority::tests::chain::uak_replacement_history_survives_winner_commit_and_wakes_after_reorg`, `authority::tests::foundation::uak_failed_rbf_fee_disposition_preserves_victims_and_terminalizes_candidate`. Command: `make integration CKB_TEST_ARGS='-c 1 RbfOrphanRecovery'`
- `rbf-cycling-attack`: `test/src/specs/tx_pool/replace.rs::RbfCyclingAttack` (T1, T2, T3, T4, T6, T8, T10) - Replacement cycling stays mutation-safe, bounded and unable to self-wake retained history. Paired units: `authority::tests::foundation::uak_replacement_history_wakes_only_on_newer_projected_availability`, `authority::tests::foundation::uak_independent_rbf_churn_never_exceeds_replacement_history_budget`, `authority::tests::foundation::uak_rbf_replaces_the_complete_descendant_closure_atomically`. Command: `make integration CKB_TEST_ARGS='-c 1 RbfCyclingAttack'`
- `rbf-proposed-success`: `test/src/specs/tx_pool/replace.rs::RbfReplaceProposedSuccess` (T1, T4, T6, T9, T10, T13) - A successful proposed replacement updates membership and template convergence once; committing its parent cannot wake retained victims while the winner still spends that output. Paired units: `authority::tests::chain::uak_chain_output_availability_respects_a_surviving_pool_spender`, `authority::tests::foundation::uak_rbf_replaces_the_complete_descendant_closure_atomically`, `authority::tests::template::uak_recovered_tree_has_normal_template_proposal_path`. Command: `make integration CKB_TEST_ARGS='-c 1 RbfReplaceProposedSuccess'`

#### `TP-BUDGET-001` - Continuous hostile-resource accounting

Generated focused command: `cargo nextest run -p ckb-tx-pool --features internal -E 'test(=authority::tests::dependency::uak_missing_growth_is_charged_or_becomes_budget_denied) | test(=authority::tests::foundation::uak_admission_must_fit_the_static_compute_envelope) | test(=authority::tests::foundation::uak_full_retained_budget_cannot_hide_the_trusted_owner) | test(=authority::tests::foundation::uak_resource_limit_failure_preserves_every_observable_fact) | test(=authority::tests::foundation::uak_resource_reference_rejects_ghost_overcharge)'`

Rust evidence:

- `authority::tests::dependency::uak_missing_growth_is_charged_or_becomes_budget_denied` (T3, T4, T8)
- `authority::tests::foundation::uak_admission_must_fit_the_static_compute_envelope` (T3, T8)
- `authority::tests::foundation::uak_full_retained_budget_cannot_hide_the_trusted_owner` (T3, T8)
- `authority::tests::foundation::uak_resource_limit_failure_preserves_every_observable_fact` (T3, T6, T8)
- `authority::tests::foundation::uak_resource_reference_rejects_ghost_overcharge` (T1, T3)

Cross-crate Rust evidence:

Generated cross-crate command: `cargo nextest run -p ckb-sync -E 'test(=relayer::tests::block_proposal_process::test_clear_expired_inflight_proposals) | test(=relayer::tests::block_proposal_process::test_no_asked) | test(=relayer::tests::block_proposal_process::test_no_unknown) | test(=relayer::tests::block_proposal_process::test_ok) | test(=relayer::tests::block_proposal_process::test_oversized_batch_is_rejected_before_relay_state_changes)'`

- `sync/src/relayer/tests/block_proposal_process.rs::relayer::tests::block_proposal_process::test_clear_expired_inflight_proposals` (T7, T8, T10) - Expired proposal requests are cleared without retaining stale relay state.
- `sync/src/relayer/tests/block_proposal_process.rs::relayer::tests::block_proposal_process::test_no_asked` (T7, T8) - An unsolicited valid proposal transaction does not enter the known projection.
- `sync/src/relayer/tests/block_proposal_process.rs::relayer::tests::block_proposal_process::test_no_unknown` (T7, T8) - A known proposal transaction is ignored without creating duplicate relay work.
- `sync/src/relayer/tests/block_proposal_process.rs::relayer::tests::block_proposal_process::test_ok` (T7, T8) - A requested valid proposal transaction preserves the accepted relay path.
- `sync/src/relayer/tests/block_proposal_process.rs::relayer::tests::block_proposal_process::test_oversized_batch_is_rejected_before_relay_state_changes` (T6, T8, T11) - RelayV3 byte bounds reject an oversized proposal batch before inflight or known state changes.

#### `TP-WORKER-001` - Capability-owned workers and progress

Generated focused command: `cargo nextest run -p ckb-tx-pool --features internal -E 'test(=authority::runtime::tests::runtime_checkout_observes_preexisting_level_without_a_wake_hint) | test(=authority::runtime::tests::runtime_compute_wakes_route_each_head_to_one_compatible_waiter_class) | test(=authority::runtime::tests::runtime_role_batons_drain_a_coalesced_preexisting_frontier) | test(=authority::runtime::tests::runtime_single_any_verifier_settles_mixed_frontier_after_batons_are_consumed) | test(=authority::tests::foundation::uak_fair_frontier_round_robins_owners_only_after_apply) | test(=authority::tests::foundation::uak_resolve_to_verify_continuation_changes_no_authority_state) | test(=authority::tests::foundation::uak_runner_cancellation_settles_one_exact_work_capability_before_exit) | test(=authority::tests::worker::uak_idle_maintenance_driver_waits_instead_of_spinning) | test(=benchmark::debug_tests::controller_repeated_idle_then_burst_never_loses_compute_wake)'`

Rust evidence:

- `authority::runtime::tests::runtime_checkout_observes_preexisting_level_without_a_wake_hint` (T2, T10)
- `authority::runtime::tests::runtime_compute_wakes_route_each_head_to_one_compatible_waiter_class` (T2, T5, T10)
- `authority::runtime::tests::runtime_role_batons_drain_a_coalesced_preexisting_frontier` (T2, T5, T10)
- `authority::runtime::tests::runtime_single_any_verifier_settles_mixed_frontier_after_batons_are_consumed` (T2, T5, T10)
- `authority::tests::foundation::uak_fair_frontier_round_robins_owners_only_after_apply` (T5, T10)
- `authority::tests::foundation::uak_resolve_to_verify_continuation_changes_no_authority_state` (T2, T5, T10)
- `authority::tests::foundation::uak_runner_cancellation_settles_one_exact_work_capability_before_exit` (T2, T10)
- `authority::tests::worker::uak_idle_maintenance_driver_waits_instead_of_spinning` (T10)
- `benchmark::debug_tests::controller_repeated_idle_then_burst_never_loses_compute_wake` (T2, T5, T10)

Process-level evidence:

- `same-lane-relay-continuation`: `test/src/specs/tx_pool/txs_relay_order.rs::TxsRelayOrder` (T2, T4, T5, T8, T10) - Same-lane continuation preserves exact work-capability ownership, dependency order and bounded fair progress. Paired units: `authority::tests::dependency::uak_popular_dependency_maintenance_has_one_edge_steps_and_key_fairness`, `authority::tests::foundation::uak_resolve_to_verify_continuation_changes_no_authority_state`, `authority::tests::foundation::uak_independent_ready_order_is_invariant_to_worker_completion_permutations`. Command: `make integration CKB_TEST_ARGS='-c 1 TxsRelayOrder'`

#### `TP-ADMIN-001` - Cause-complete administration and peer revocation

Generated focused command: `cargo nextest run -p ckb-tx-pool --features internal -E 'test(=authority::tests::ban::saturation_evicts_the_oldest_fence_and_keeps_a_hard_bound) | test(=authority::tests::foundation::uak_accepted_expiry_uses_stable_deadlines_and_expires_the_full_closure) | test(=authority::tests::foundation::uak_clear_pipeline_preserves_accepted_and_invalidates_active_work) | test(=authority::tests::foundation::uak_local_non_remote_preaccepted_removal_does_not_release_relay_state) | test(=authority::tests::foundation::uak_peer_revocation_removes_only_preaccepted_ingress_owners) | test(=authority::tests::foundation::uak_remote_expiry_is_a_bounded_derived_transition_and_allows_refetch) | test(=authority::tests::foundation::uak_runtime_clear_scopes_and_snapshot_pairing_are_indivisible) | test(=authority::tests::ingress::uak_delayed_revoked_remote_ingress_commits_a_later_filter_release) | test(=authority::tests::ingress::uak_peer_fence_saturation_revalidates_the_oldest_delayed_session)'`

Rust evidence:

- `authority::tests::ban::saturation_evicts_the_oldest_fence_and_keeps_a_hard_bound` (T8, T10)
- `authority::tests::foundation::uak_accepted_expiry_uses_stable_deadlines_and_expires_the_full_closure` (T3, T4, T6)
- `authority::tests::foundation::uak_clear_pipeline_preserves_accepted_and_invalidates_active_work` (T1, T2, T3, T6)
- `authority::tests::foundation::uak_local_non_remote_preaccepted_removal_does_not_release_relay_state` (T7, T11)
- `authority::tests::foundation::uak_peer_revocation_removes_only_preaccepted_ingress_owners` (T1, T3, T7)
- `authority::tests::foundation::uak_remote_expiry_is_a_bounded_derived_transition_and_allows_refetch` (T1, T3, T7, T10)
- `authority::tests::foundation::uak_runtime_clear_scopes_and_snapshot_pairing_are_indivisible` (T1, T2, T3, T6, T9, T12)
- `authority::tests::ingress::uak_delayed_revoked_remote_ingress_commits_a_later_filter_release` (T1, T6, T7, T10)
- `authority::tests::ingress::uak_peer_fence_saturation_revalidates_the_oldest_delayed_session` (T3, T8, T10, T11)

Cross-crate Rust evidence:

Generated cross-crate command: `cargo nextest run -p ckb-sync -E 'test(=relayer::tests::tx_verification_results::rejected_tx_can_be_requested_again_from_another_peer)'`

- `sync/src/relayer/tests/tx_verification_results.rs::relayer::tests::tx_verification_results::rejected_tx_can_be_requested_again_from_another_peer` (T7, T10) - A generation reset followed by delayed Remote ingress is closed by the later exact Reject, so the same raw transaction becomes requestable from another peer.

Process-level evidence:

- `truncate-clear-order`: `test/src/specs/rpc/truncate.rs::RpcTruncate` (T1, T3, T6, T9, T10, T12) - A clear issued by truncate after the chain transition is ordered after detached recovery and leaves one empty generation at the truncated snapshot. Paired units: `service::controller::tests::generation_clear_cannot_overtake_a_prior_chain_transition`, `authority::tests::foundation::uak_runtime_clear_scopes_and_snapshot_pairing_are_indivisible`. Command: `make integration CKB_TEST_ARGS='-c 1 RpcTruncate'`

#### `TP-EFFECT-001` - Atomic bounded effects and publication

Generated focused command: `cargo nextest run -p ckb-tx-pool --features internal -E 'test(=authority::tests::effect::uak_effect_full_preserves_ready_owner_and_charge) | test(=authority::tests::effect::uak_effect_lease_preserves_sequence_and_charge) | test(=authority::tests::effect::uak_generation_reset_coalesces_and_retain_never_resurrects_an_old_reset) | test(=authority::tests::effect::uak_production_effect_sizing_constructively_covers_non_rebuildable_shapes) | test(=authority::tests::effect::uak_remote_missing_wait_and_parent_request_share_one_backpressured_apply) | test(=authority::tests::publisher::uak_publisher_relay_disconnect_disposes_and_drains_the_authority_head)'`

Rust evidence:

- `authority::tests::effect::uak_effect_full_preserves_ready_owner_and_charge` (T1, T3, T6, T7)
- `authority::tests::effect::uak_effect_lease_preserves_sequence_and_charge` (T2, T3, T7)
- `authority::tests::effect::uak_generation_reset_coalesces_and_retain_never_resurrects_an_old_reset` (T7, T10)
- `authority::tests::effect::uak_production_effect_sizing_constructively_covers_non_rebuildable_shapes` (T3, T7, T8)
- `authority::tests::effect::uak_remote_missing_wait_and_parent_request_share_one_backpressured_apply` (T4, T7, T10)
- `authority::tests::publisher::uak_publisher_relay_disconnect_disposes_and_drains_the_authority_head` (T7, T10)

Cross-crate Rust evidence:

Generated cross-crate command: `cargo nextest run -p ckb-sync -E 'test(=relayer::tests::tx_verification_results::committed_tx_result_is_consumed_without_relay_peers) | test(=relayer::tests::tx_verification_results::rejected_tx_can_be_requested_again_from_another_peer)'`

- `sync/src/relayer/tests/tx_verification_results.rs::relayer::tests::tx_verification_results::committed_tx_result_is_consumed_without_relay_peers` (T7, T10) - Committed tx-pool results update the local relay projection even with no connected relay peer and retain bounded later broadcast intent.
- `sync/src/relayer/tests/tx_verification_results.rs::relayer::tests::tx_verification_results::rejected_tx_can_be_requested_again_from_another_peer` (T7, T10) - A generation reset followed by delayed Remote ingress is closed by the later exact Reject, so the same raw transaction becomes requestable from another peer.

Process-level evidence:

- `multi-parent-orphan-frontier`: `test/src/specs/tx_pool/orphan_tx.rs::TxPoolOrphanReverse` (T4, T6, T7, T10) - A complete multi-parent missing frontier and its relay request commit in one bounded transition. Paired units: `authority::tests::dependency::uak_direct_parent_acceptance_publishes_output_availability_atomically`, `authority::tests::effect::uak_remote_missing_wait_and_parent_request_share_one_backpressured_apply`. Command: `make integration CKB_TEST_ARGS='-c 1 TxPoolOrphanReverse'`

#### `TP-REORG-001` - Reliable atomic chain reconciliation

Generated focused command: `cargo nextest run -p ckb-tx-pool --features internal -E 'test(=authority::tests::chain::uak_chain_boundary_closes_ordered_backpressure_without_open_plan_errors) | test(=authority::tests::chain::uak_chain_commit_removes_a_parent_without_stranding_its_surviving_child) | test(=authority::tests::chain::uak_detached_parent_and_accepted_child_recover_parent_first) | test(=authority::tests::chain::uak_runtime_chain_boundary_reconciles_indexed_gap_against_paired_snapshot) | test(=service::controller::tests::authoritative_reorg_delivery_is_independent_of_rpc_readiness) | test(=service::controller::tests::closed_reorg_consumer_fails_without_waiting) | test(=service::controller::tests::generation_clear_cannot_overtake_a_prior_chain_transition)'`

Rust evidence:

- `authority::tests::chain::uak_chain_boundary_closes_ordered_backpressure_without_open_plan_errors` (T6, T9, T10)
- `authority::tests::chain::uak_chain_commit_removes_a_parent_without_stranding_its_surviving_child` (T4, T10)
- `authority::tests::chain::uak_detached_parent_and_accepted_child_recover_parent_first` (T1, T4, T6, T9)
- `authority::tests::chain::uak_runtime_chain_boundary_reconciles_indexed_gap_against_paired_snapshot` (T5, T6, T9, T12)
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

Generated focused command: `cargo nextest run -p ckb-tx-pool --features internal -E 'test(=authority::tests::read::uak_persistence_receipt_is_coherent_and_parent_first) | test(=authority::tests::service::uak_service_persists_one_coherent_authority_receipt_outside_the_guard) | test(=persisted::tests::persistence_loader_accepts_legacy_v1_vector) | test(=persisted::tests::persistence_loader_rejects_an_unrepresentable_read_bound_before_io) | test(=persisted::tests::persistence_v2_rejects_oversized_file_before_reading_payload) | test(=persisted::tests::persistence_writer_admits_only_one_snapshot_owner)'`

Rust evidence:

- `authority::tests::read::uak_persistence_receipt_is_coherent_and_parent_first` (T4, T9, T12)
- `authority::tests::service::uak_service_persists_one_coherent_authority_receipt_outside_the_guard` (T9, T10, T12)
- `persisted::tests::persistence_loader_accepts_legacy_v1_vector` (T11, T12)
- `persisted::tests::persistence_loader_rejects_an_unrepresentable_read_bound_before_io` (T8, T11)
- `persisted::tests::persistence_v2_rejects_oversized_file_before_reading_payload` (T8, T11)
- `persisted::tests::persistence_writer_admits_only_one_snapshot_owner` (T10, T12)

#### `TP-QUERY-001` - Coherent public projections

Generated focused command: `cargo nextest run -p ckb-tx-pool --features internal -E 'test(=authority::tests::query::uak_compact_receipt_releases_authority_before_storage_lookup) | test(=authority::tests::query::uak_owned_pool_queries_share_one_status_and_aggregate_cut) | test(=authority::tests::query::uak_owned_transaction_query_hides_replacement_history_and_reports_minimum_fee) | test(=authority::tests::read::uak_query_never_splices_two_authority_cuts)'`

Rust evidence:

- `authority::tests::query::uak_compact_receipt_releases_authority_before_storage_lookup` (T10, T12)
- `authority::tests::query::uak_owned_pool_queries_share_one_status_and_aggregate_cut` (T12)
- `authority::tests::query::uak_owned_transaction_query_hides_replacement_history_and_reports_minimum_fee` (T12)
- `authority::tests::read::uak_query_never_splices_two_authority_cuts` (T9, T12)

Process-level evidence:

- `failed-rbf-terminal`: `test/src/specs/tx_pool/replace.rs::RbfContainInvalidInput` (T1, T3, T6, T7, T8, T12) - A policy-rejected RBF candidate publishes per-hash rejection while preserving every accepted owner and creating no uncharged conflict-history residency. Paired units: `authority::tests::foundation::uak_terminal_outcome_and_effect_commit_together`, `authority::tests::foundation::uak_failed_rbf_fee_disposition_preserves_victims_and_terminalizes_candidate`, `authority::tests::foundation::uak_independent_rbf_churn_never_exceeds_replacement_history_budget`, `authority::tests::query::uak_owned_transaction_query_hides_replacement_history_and_reports_minimum_fee`. Command: `make integration CKB_TEST_ARGS='-c 1 RbfContainInvalidInput'`

#### `TP-DEFECT-001` - Rust-native ordinary failure boundary

Generated focused command: `cargo nextest run -p ckb-tx-pool --features internal -E 'test(=authority::tests::foundation::uak_counter_exhaustion_is_typed_and_mutation_free) | test(=authority::tests::ingress::uak_retained_ingress_boundary_keeps_legal_pressure_out_of_fail_stop) | test(=authority::tests::resolver::uak_duplicate_inputs_are_a_malformed_outcome_not_an_authority_fault) | test(=authority::tests::service::uak_candidate_uncle_degradation_remains_outside_authority_invalidity) | test(=authority::tests::service::uak_only_integrity_faults_invalidate_a_generation) | test(=authority::tests::service::uak_operational_failure_classes_follow_ownership_boundaries) | test(=authority::tests::service::uak_ordered_chain_update_has_no_droppable_operational_error) | test(=authority::tests::service::uak_recent_reject_encoding_failure_remains_outside_authority_invalidity) | test(=authority::tests::topology::uak_topology_forbids_persistence_only_after_authority_integrity_loss) | test(=block_assembler::candidate_uncles::tests::candidate_uncle_source_version_exhaustion_is_typed_and_mutation_free)'`

Rust evidence:

- `authority::tests::foundation::uak_counter_exhaustion_is_typed_and_mutation_free` (T2, T6, T8)
- `authority::tests::ingress::uak_retained_ingress_boundary_keeps_legal_pressure_out_of_fail_stop` (T3, T8)
- `authority::tests::resolver::uak_duplicate_inputs_are_a_malformed_outcome_not_an_authority_fault` (T8, T11)
- `authority::tests::service::uak_candidate_uncle_degradation_remains_outside_authority_invalidity` (T6, T10, T13)
- `authority::tests::service::uak_only_integrity_faults_invalidate_a_generation` (T6, T7, T10)
- `authority::tests::service::uak_operational_failure_classes_follow_ownership_boundaries` (T7, T10)
- `authority::tests::service::uak_ordered_chain_update_has_no_droppable_operational_error` (T6, T9, T10)
- `authority::tests::service::uak_recent_reject_encoding_failure_remains_outside_authority_invalidity` (T7, T10)
- `authority::tests::topology::uak_topology_forbids_persistence_only_after_authority_integrity_loss` (T6, T7, T10)
- `block_assembler::candidate_uncles::tests::candidate_uncle_source_version_exhaustion_is_typed_and_mutation_free` (T6, T8, T13)

Process-level evidence:

- `local-test-rpc-direct`: `test/src/specs/tx_pool/local_test_submission.rs::LocalTestSubmissionIsDirect` (T1, T2, T3, T6, T7, T8, T11) - Local admission remains direct and atomic while TestAccept remains read-only under the same policy. Paired units: `authority::tests::foundation::uak_direct_local_admission_moves_from_absent_to_accepted_in_one_apply`, `authority::tests::chain::uak_final_admission_receipt_is_stale_after_chain_view_aba`, `authority::tests::resolver::uak_duplicate_inputs_are_a_malformed_outcome_not_an_authority_fault`. Command: `make integration CKB_TEST_ARGS='-c 1 LocalTestSubmissionIsDirect'`

#### `TP-POOL-001` - Atomic accepted membership graph

Generated focused command: `cargo nextest run -p ckb-tx-pool --features internal -E 'test(=authority::tests::foundation::uak_capacity_eviction_removes_one_complete_causal_component) | test(=authority::tests::foundation::uak_causal_diamond_is_projection_equivalent_for_every_arrival_order) | test(=authority::tests::foundation::uak_membership_projects_one_spender_and_one_causal_graph) | test(=authority::tests::foundation::uak_status_reconcile_updates_count_and_eviction_projection_once)'`

Rust evidence:

- `authority::tests::foundation::uak_capacity_eviction_removes_one_complete_causal_component` (T3, T4, T6, T8)
- `authority::tests::foundation::uak_causal_diamond_is_projection_equivalent_for_every_arrival_order` (T4, T6)
- `authority::tests::foundation::uak_membership_projects_one_spender_and_one_causal_graph` (T1, T4, T6, T12)
- `authority::tests::foundation::uak_status_reconcile_updates_count_and_eviction_projection_once` (T5, T6, T12)

Process-level evidence:

- `accepted-descendants-after-parent-reorg`: `test/src/specs/tx_pool/pool_reconcile.rs::PoolResolveConflictAfterReorg` (T1, T3, T4, T6, T9, T10) - A detached accepted parent and surviving descendant closure reconcile as one valid owner generation. Paired units: `authority::tests::dependency::uak_parent_terminalization_cannot_strand_trusted_child`, `authority::tests::chain::uak_detached_parent_and_accepted_child_recover_parent_first`, `authority::tests::foundation::uak_capacity_eviction_removes_one_complete_causal_component`. Command: `make integration CKB_TEST_ARGS='-c 1 PoolResolveConflictAfterReorg'`
- `conditional-reader-fanout`: `test/src/specs/tx_pool/limit.rs::TxPoolLimitAncestorCount` (T3, T4, T6, T8, T10, T13) - A high-fanout dependency shape remains bounded without corrupting accepted causal limits. Paired units: `authority::tests::dependency::uak_popular_dependency_maintenance_has_one_edge_steps_and_key_fairness`, `authority::tests::foundation::uak_capacity_eviction_removes_one_complete_causal_component`, `block_assembler::candidate_uncles::tests::full_container_keeps_a_hard_global_bound`. Command: `make integration CKB_TEST_ARGS='-c 1 TxPoolLimitAncestorCount'`
- `rbf-cell-deps`: `test/src/specs/tx_pool/replace.rs::RbfCellDepsCheck` (T1, T4, T6, T11, T12) - A replacement depending on any victim is rejected before graph mutation. Paired units: `authority::tests::foundation::uak_rbf_dependency_on_any_victim_is_mutation_free`, `authority::tests::foundation::uak_membership_projects_one_spender_and_one_causal_graph`. Command: `make integration CKB_TEST_ARGS='-c 1 RbfCellDepsCheck'`

#### `TP-TEMPLATE-001` - Concurrent versioned template convergence

Generated focused command: `cargo nextest run -p ckb-tx-pool --features internal -E 'test(=authority::tests::packing::uak_template_packer_selects_an_exact_fit_cpfp_package_parent_first) | test(=authority::tests::template::uak_recovered_tree_has_normal_template_proposal_path) | test(=authority::tests::template::uak_template_read_receipt_shares_order_and_complete_resolved_payload) | test(=authority::tests::template_driver::uak_template_driver_full_priority_and_partial_occ_use_one_output_revision) | test(=authority::tests::template_driver::uak_template_source_probe_skips_irrelevant_population_captures) | test(=block_assembler::tests::block_template_preserves_consensus_uncle_limit_above_u8) | test(=block_assembler::tests::optional_content_uses_one_budget_and_filters_only_published_conflicts)'`

Rust evidence:

- `authority::tests::packing::uak_template_packer_selects_an_exact_fit_cpfp_package_parent_first` (T4, T8, T13)
- `authority::tests::template::uak_recovered_tree_has_normal_template_proposal_path` (T4, T9, T10, T13)
- `authority::tests::template::uak_template_read_receipt_shares_order_and_complete_resolved_payload` (T4, T9, T12, T13)
- `authority::tests::template_driver::uak_template_driver_full_priority_and_partial_occ_use_one_output_revision` (T9, T10, T13)
- `authority::tests::template_driver::uak_template_source_probe_skips_irrelevant_population_captures` (T10, T13)
- `block_assembler::tests::block_template_preserves_consensus_uncle_limit_above_u8` (T8, T11, T13)
- `block_assembler::tests::optional_content_uses_one_budget_and_filters_only_published_conflicts` (T8, T13)

Process-level evidence:

- `async-uncle-candidate-publication`: `test/src/specs/mining/uncle.rs::UncleInheritFromForkUncle` (T8, T9, T10, T13) - Candidate-uncles remain asynchronously publishable without violating reset/full priority or proposal conflict filtering. Paired units: `service::controller::tests::authoritative_reorg_delivery_is_independent_of_rpc_readiness`, `authority::tests::template_driver::uak_template_driver_full_priority_and_partial_occ_use_one_output_revision`, `block_assembler::tests::optional_content_uses_one_budget_and_filters_only_published_conflicts`. Command: `make integration CKB_TEST_ARGS='-c 1 UncleInheritFromForkUncle'`
- `cell-dep-arrival-order`: `test/src/specs/tx_pool/dead_cell_deps.rs::CellBeingCellDepAndSpentInSameBlockTestGetBlockTemplate` (T4, T6, T11, T12, T13) - A selected cell-dep reader remains ordered before the spender regardless of arrival order. Paired units: `authority::tests::foundation::uak_coupled_membership_requires_exact_positive_input_evidence`, `authority::tests::template::uak_template_read_receipt_shares_order_and_complete_resolved_payload`. Command: `make integration CKB_TEST_ARGS='-c 1 CellBeingCellDepAndSpentInSameBlockTestGetBlockTemplate'`
- `normal-mining-reorg-tree`: `test/src/specs/tx_pool/reorg_recovers_dependent.rs::ReorgRecoversDependentPendingTree` (T5, T9, T10, T12, T13) - A recovered dependent tree is re-proposed and mined through the normal template path after reorg. Paired units: `authority::tests::chain::uak_runtime_chain_boundary_reconciles_indexed_gap_against_paired_snapshot`, `authority::tests::template_driver::uak_template_driver_full_priority_and_partial_occ_use_one_output_revision`. Command: `make integration CKB_TEST_ARGS='-c 1 ReorgRecoversDependentPendingTree'`
- `rbf-proposed-success`: `test/src/specs/tx_pool/replace.rs::RbfReplaceProposedSuccess` (T1, T4, T6, T9, T10, T13) - A successful proposed replacement updates membership and template convergence once; committing its parent cannot wake retained victims while the winner still spends that output. Paired units: `authority::tests::chain::uak_chain_output_availability_respects_a_surviving_pool_spender`, `authority::tests::foundation::uak_rbf_replaces_the_complete_descendant_closure_atomically`, `authority::tests::template::uak_recovered_tree_has_normal_template_proposal_path`. Command: `make integration CKB_TEST_ARGS='-c 1 RbfReplaceProposedSuccess'`
- `rbf-proposed-template-refresh`: `test/src/specs/tx_pool/replace.rs::RbfRejectReplaceProposed` (T1, T4, T6, T9, T13) - Rejected proposed replacement is mutation-free and cannot publish a stale template generation. Paired units: `authority::tests::foundation::uak_rbf_replaces_the_complete_descendant_closure_atomically`, `authority::tests::template_driver::uak_template_driver_full_priority_and_partial_occ_use_one_output_revision`. Command: `make integration CKB_TEST_ARGS='-c 1 RbfRejectReplaceProposed'`

#### `TP-IDENTITY-001` - Exact transaction and evidence identity

Generated focused command: `cargo nextest run -p ckb-tx-pool --features internal -E 'test(=authority::tests::foundation::uak_short_id_collision_cannot_alias_primary_identity) | test(=authority::tests::foundation::uak_tx_pool_chain_backed_finalization_rejects_pool_origin_cells_by_construction) | test(=authority::tests::resolver::uak_verification_cache_lookup_cannot_substitute_a_nearby_request) | test(=authority::tests::resolver::uak_verification_request_binds_environment_rules_and_witness_cache_key) | test(=authority::tests::validation::uak_final_validation_rejects_a_mixed_authority_snapshot_cut) | test(=authority::tests::validation::uak_same_tip_unproven_location_is_rejected_not_treated_as_pool_origin)'`

Rust evidence:

- `authority::tests::foundation::uak_short_id_collision_cannot_alias_primary_identity` (T1, T11)
- `authority::tests::foundation::uak_tx_pool_chain_backed_finalization_rejects_pool_origin_cells_by_construction` (T2, T9, T11)
- `authority::tests::resolver::uak_verification_cache_lookup_cannot_substitute_a_nearby_request` (T11)
- `authority::tests::resolver::uak_verification_request_binds_environment_rules_and_witness_cache_key` (T11)
- `authority::tests::validation::uak_final_validation_rejects_a_mixed_authority_snapshot_cut` (T9, T11)
- `authority::tests::validation::uak_same_tip_unproven_location_is_rejected_not_treated_as_pool_origin` (T9, T11)

#### `TP-PERF-001` - Bounded work and preserved concurrency

Generated focused command: `cargo nextest run -p ckb-tx-pool --features internal -E 'test(=authority::tests::foundation::uak_checkout_attack_work_is_bounded_by_owner_heads_and_active_slots) | test(=authority::tests::foundation::uak_independent_ready_order_is_invariant_to_worker_completion_permutations) | test(=authority::tests::relay::uak_relay_mailbox_accounting_mismatch_rebuilds_instead_of_saturating) | test(=authority::tests::relay::uak_relay_mailbox_bounds_oversized_parent_detail_without_blocking) | test(=benchmark::debug_tests::profile_span_counters_observe_only_the_active_registered_window) | test(=benchmark::debug_tests::profile_span_counters_reject_an_unregistered_target_span) | test(=block_assembler::candidate_uncles::tests::full_container_keeps_a_hard_global_bound) | test(=component::tests::score_key::ancestor_score_order_is_deterministic_at_extreme_weights) | test(=service::controller::tests::asynchronous_network_calls_fail_fast_when_the_controller_channel_is_full)'`

Rust evidence:

- `authority::tests::foundation::uak_checkout_attack_work_is_bounded_by_owner_heads_and_active_slots` (T3, T5, T8)
- `authority::tests::foundation::uak_independent_ready_order_is_invariant_to_worker_completion_permutations` (T5, T8, T10)
- `authority::tests::relay::uak_relay_mailbox_accounting_mismatch_rebuilds_instead_of_saturating` (T7, T8, T10)
- `authority::tests::relay::uak_relay_mailbox_bounds_oversized_parent_detail_without_blocking` (T7, T8, T10)
- `benchmark::debug_tests::profile_span_counters_observe_only_the_active_registered_window` (T8)
- `benchmark::debug_tests::profile_span_counters_reject_an_unregistered_target_span` (T8)
- `block_assembler::candidate_uncles::tests::full_container_keeps_a_hard_global_bound` (T8, T13)
- `component::tests::score_key::ancestor_score_order_is_deterministic_at_extreme_weights` (T8, T13)
- `service::controller::tests::asynchronous_network_calls_fail_fast_when_the_controller_channel_is_full` (T8, T10)

Process-level evidence:

- `conditional-reader-fanout`: `test/src/specs/tx_pool/limit.rs::TxPoolLimitAncestorCount` (T3, T4, T6, T8, T10, T13) - A high-fanout dependency shape remains bounded without corrupting accepted causal limits. Paired units: `authority::tests::dependency::uak_popular_dependency_maintenance_has_one_edge_steps_and_key_fairness`, `authority::tests::foundation::uak_capacity_eviction_removes_one_complete_causal_component`, `block_assembler::candidate_uncles::tests::full_container_keeps_a_hard_global_bound`. Command: `make integration CKB_TEST_ARGS='-c 1 TxPoolLimitAncestorCount'`
- `same-lane-relay-continuation`: `test/src/specs/tx_pool/txs_relay_order.rs::TxsRelayOrder` (T2, T4, T5, T8, T10) - Same-lane continuation preserves exact work-capability ownership, dependency order and bounded fair progress. Paired units: `authority::tests::dependency::uak_popular_dependency_maintenance_has_one_edge_steps_and_key_fairness`, `authority::tests::foundation::uak_resolve_to_verify_continuation_changes_no_authority_state`, `authority::tests::foundation::uak_independent_ready_order_is_invariant_to_worker_completion_permutations`. Command: `make integration CKB_TEST_ARGS='-c 1 TxsRelayOrder'`

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
