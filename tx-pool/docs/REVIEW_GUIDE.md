# Tx-Pool Test-Driven Review Guide

This guide is the reviewer entry point for tx-pool changes. It translates the
T1–T13 proof obligations in [`ARCHITECTURE.md`](ARCHITECTURE.md) into stable
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
3. Run the row's minimum command and inspect its focused negative assertions.
   Test names are stable review anchors; renaming or deleting one is an explicit
   evidence change, not cleanup.
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
one authority/location at any instant, and every resident resource is
continuously charged. The old multi-queue/orphan/conflict ownership and
accepted rollback journal have been replaced by the six-location
`PrePoolKernel` and accepted `PoolMutationPlan`; Recovery is a source, not a
seventh state. Stable effects use one statically partitioned `EffectJournal`
with no dynamic reservations. `TxPool` is the sole accepted-state authority.
Derived indexes and worker borrows never become owners.

## Cross-authority gate

Apply this gate whenever a change touches more than one of `PrePoolKernel`,
`TxPool`, `EffectJournal`, reorg recovery, persistence or block assembler:

- Identify the linearization point and prove there is no visible ownership gap
  or overlap.
- Write the lock/resource order explicitly. A capacity hint owns no state and
  is released before `TxPool -> PrePoolKernel -> EffectJournal`; no authority
  guard spans work/I/O/await. Detached replay is ordinary charged Recovery
  source data in the six-state kernel; there is no recovery-wide lock or
  handler-owned retained vector.
- Prove every legal outcome is `Apply`, typed `Reject`, `Backpressure`, `Stale`
  or `Duplicate`, and every detectable structural fault is typed before Apply.
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
- Re-check RPC visibility, reorg replay, persistence ordering and normal
  `get_block_template` proposal/commit liveness.

## Command tiers

For a focused change, run the minimum commands in every selected row, then:

```text
python3 tx-pool/scripts/check_docs.py
python3 tx-pool/scripts/check_review_guide.py
python3 tx-pool/scripts/check_test_layout.py
python3 tx-pool/scripts/check_security_manifest.py
cargo nextest run -p ckb-tx-pool --features internal
cargo clippy -p ckb-tx-pool --all-targets --features internal -- -D warnings
```

When `TP-ADMIN-001` changes the relay projection boundary, also run:

```text
cargo nextest run -p ckb-sync rejected_tx_can_be_requested_again_from_another_peer committed_tx_result_is_consumed_without_relay_peers
```

These cross-crate regressions prove that a terminal Reject removes both the
known-filter and pending-broadcast projections, and that the same transaction
can subsequently be requested from a different peer even if the original
tx-pool result was consumed while no relay peer was connected.

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

## Registered behaviors and evidence

<!-- BEGIN GENERATED: TX_POOL_BEHAVIORS -->

### Managed process suite

The 16 focused security anchors are the minimum process gate for the mapped behavior rows:

`make integration CKB_TEST_ARGS='-c 1 CellBeingCellDepAndSpentInSameBlockTestGetBlockTemplate TxPoolLimitAncestorCount ReorgRecoversDependentPendingTree ReorgRecoversDependentChain ReorgRecoversDependentTxs PoolResolveConflictAfterReorg RbfRejectReplaceProposed RbfOrphanRecovery RbfBasic RbfReplaceProposedSuccess RbfConcurrency RbfCyclingAttack RbfCellDepsCheck TxPoolOrphanReverse LocalTestSubmissionIsDirect UncleInheritFromForkUncle'`

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

| ID | Change surfaces | Required behavior | Hostile/failure case | Invariants | Reviewer gate | Performance bound |
|---|---|---|---|---|---|---|
| `TP-OWN-001` Single pre-pool ownership | `tx-pool/src/component/pre_pool`<br>`tx-pool/src/service/pipeline_ops.rs`<br>`tx-pool/src/service/workers.rs` | An admitted transaction has one full-hash PrePoolKernel entry, one of the frozen six locations and one globally non-reused version until an atomic handoff transfers sole authority to TxPool. | A stale worker, duplicate admission, failed transition or ABA remove/readmit race must not create two owners, resurrect an old payload or silently erase the current owner. | T1, T2, T3, T4, T5, T6, T7, T8, T10, T11, T13 | - Does every transition consume exactly the state and lease it proves current?<br>- Are every queue, deadline, dependency and conflict structure derived indexes rather than payload owners?<br>- Does failure restore the old owner or publish one explicit terminal outcome? | No second owner map, compensating queue, global post-transition scan or extra hot-path lock. |
| `TP-COMMIT-001` Authoritative commit and handoff | `tx-pool/src/process/submit`<br>`tx-pool/src/component/pre_pool/commit.rs`<br>`tx-pool/src/pool.rs` | The TxPool write guard is the only final membership/RBF sequencer. It builds one immutable AdmissionPlan containing the accepted PoolMutationPlan, matching versioned kernel handoff, exact effect batch and template receipt; one total Apply moves the logical owner and journals those committed effects. Tx-pool final admission may reuse positive resolved-cell evidence only per cell, after the mutable pool overlay, when the resolution and current tip hashes match and transaction_info proves chain provenance. | Concurrent commits, stale leases, rejected RBF, capacity failure, accepted-duplicate acknowledgement racing clear/reorg, or a structural defect must not expose a pool/kernel ownership gap, strand Ready work, or publish success for an absent/unapplied owner. | T1, T2, T3, T4, T5, T6, T7, T8, T9, T11 | - Is every final fee/conflict decision recomputed under the pool write guard?<br>- Is same-tip resolved-cell evidence confined to tx-pool admission, chain-provenanced per cell, and excluded from removal-history and block/consensus validation?<br>- Can any ordinary error occur after accepted-pool Apply begins?<br>- Is an uncertain authoritative mutation escalated instead of downgraded to a transaction reject? | Reuse the existing pool sequencer; do not add a normal-path recovery lock, second commit queue or population-sized reconciliation. |
| `TP-RBF-001` Deterministic mutation-free RBF planning | `tx-pool/src/process/submit/rbf_commit.rs`<br>`tx-pool/src/component/pre_pool/commit.rs`<br>`tx-pool/src/component/pre_pool/wait.rs` | Only verified candidates participate in deterministic conflict ordering, while TxPool recomputes the complete replacement closure and both fee gates before atomic victim displacement. | An under-fee, multi-input, dep-group, self-evicted or concurrent candidate must not preempt through speculative state; every failed replacement must leave the complete original pool unchanged. | T1, T2, T3, T4, T5, T6, T7, T8, T10, T11, T12, T13 | - Is pre-pool Ready ordering still provisional rather than an admission verdict?<br>- Are all input and expanded dependency conflicts included in the immutable removal union?<br>- Does every failed path preserve original statuses, accounting and descendant order? | Conflict work stays within indexed bounded cohorts and immutable mutation plans; no full-pool scan under the write guard. |
| `TP-DEP-001` Causal dependency graph | `tx-pool/src/component/pre_pool/wait.rs`<br>`tx-pool/src/component/pre_pool/lifecycle.rs`<br>`tx-pool/src/resolved_tx.rs`<br>`tx-pool/src/component/links.rs`<br>`tx-pool/src/pool_cell.rs`<br>`tx-pool/src/component/tx_selector.rs` | Raw, resolved and accepted exact dependency keys—including the complete direct missing-cell frontier, headers and expanded dep-group members—remain one canonical primary fact: availability wakes children and definitive loss invalidates them atomically while by-parent is only a derived projection. | A cell provider reporting only its first miss, late-discovered parents, conditional reader/spender cycles, reversed arrival order, stale resolved children or parent replacement must not strand a child, make RelayV3 reject an unrequested parent, lose its wake edge or let a template commit an invalid order. | T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13 | - Are input, cell-dep, header-dep and expanded dep-group roles intentionally distinguished?<br>- Does parent success/failure update reverse edges, accounting and child location in one transition?<br>- Are cascade size and maintenance work explicitly bounded? | Use bounded indexed parent/child buckets and maintenance slices; never poll all waiting transactions or scan the pool for dependents. |
| `TP-CACHE-001` Conflict-history Wait ownership and wakeup | `tx-pool/src/component/pre_pool/wait.rs`<br>`tx-pool/src/component/pre_pool/lifecycle.rs`<br>`tx-pool/src/component/pre_pool/stored_entry.rs`<br>`tx-pool/src/component/pre_pool/runtime.rs`<br>`tx-pool/src/service/pipeline_ops.rs`<br>`tx-pool/src/process/reorg.rs` | Historical conflicts are ordinary bounded PrePoolKernel Wait entries until an exact dependency becomes available in the post-mutation TxPool/Snapshot overlay; unchanged history observes the next dependency level, while a victim retained by the same atomic cohort is sealed at that cohort's post-Apply observation cut and cannot self-wake. Waking changes the same primary owner atomically and is version-safe, while RPC projects retained conflict history through recent-reject rather than as live Pending work. | A release observed before another parent becomes live, a newly retained victim observing its own replacement release, an output created and consumed in one attached branch, duplicate metadata enrichment, remote conflict pinning or high-fanout input must not lose the only future wake, create a false wake, overwrite the root rejection, masquerade as Pending, retain epoch history without a waiter, duplicate ownership or cause unbounded pool-lock work. | T1, T2, T3, T4, T5, T6, T7, T8, T10, T11, T13 | - Does Wait retain ownership until an exact dependency epoch changes?<br>- Does the cohort seal distinguish an unchanged historical waiter from a conflict victim created by that same Apply, without hand-coded call-site ordering?<br>- Does every chain/pool availability edge re-arm a previously examined entry only when the resulting authoritative overlay makes that exact dependency available?<br>- Are discovery, generations and fanout work bounded and fair? | Bound history count/bytes and process indexed recovery in fixed fair slices outside population-sized scans. |
| `TP-BUDGET-001` Continuous hostile-state accounting | `tx-pool/src/component/pre_pool/mod.rs`<br>`tx-pool/src/component/pre_pool/runtime.rs`<br>`tx-pool/src/service/effects.rs` | Global and per-peer count, bytes and active-work budgets continuously charge payload and conservative metadata in every resident state, including bounded terminal effects. | Parking, invalidation, reservation, peer churn or an oversized displacement plan must not refund resident state, including the remote/per-peer charge of conflict history, evict unrelated stronger work or mutate before proving the bound. | T1, T2, T3, T4, T6, T7, T8, T10, T11, T13 | - Is every owner charged if and only if it is resident?<br>- Are count, bytes, graph edges, victims and active work all bounded before mutation?<br>- Does an impossible peer admission fail before global eviction planning? | Budget checks and victim selection use maintained bounded indexes; no attacker-sized repair on the admission hot path. |
| `TP-WORKER-001` Level-triggered executable readiness | `tx-pool/src/component/pre_pool/queue.rs`<br>`tx-pool/src/component/pre_pool/runtime.rs`<br>`tx-pool/src/service/workers.rs`<br>`tx-pool/src/service/builder.rs`<br>`tx-pool/src/service/stages/runner.rs` | Readiness is derived after each transition from the authoritative capability-aware checkout predicate; internal job computation returns a typed settlement and re-arms retained work, while loss of worker/command authority requests controlled service shutdown rather than panic recovery or model restart. | A capped peer backlog must not self-wake into mutex-starving livelock; a small-only worker must not consume the only wake for large work; cancellation, zero workers, a consumed Notify around a typed job failure or a permanently closed command channel must neither strand executable work nor create a respawn loop. | T1, T2, T3, T4, T7, T8, T10, T11, T13 | - Does notification mean at least one worker of this capability can execute now?<br>- Can a consumed wake be reconstructed from authoritative state after subscribe or a typed job failure?<br>- Does any internal worker use panic plus catch_unwind as control flow instead of a typed result?<br>- Does cancellation stop checkout before another self-sustaining wake loop begins? | Readiness checks inspect bounded owner heads/caps only; worker count is configuration-bounded; no restart generation, polling loop, per-item task or queue-wide scan. |
| `TP-ADMIN-001` Administrative and hostile-peer terminalization | `tx-pool/src/service.rs`<br>`tx-pool/src/service/pipeline_ops.rs`<br>`tx-pool/src/process/classify.rs`<br>`tx-pool/src/component/recent_reject.rs`<br>`tx-pool/src/process/post_process.rs`<br>`tx-pool/src/process/submit/mod.rs`<br>`tx-pool/src/process/submit/rbf_commit.rs`<br>`tx-pool/src/component/pre_pool/commit.rs`<br>`sync/src/relayer/mod.rs`<br>`sync/src/types/mod.rs` | Ban, expiry and malformed-input policy terminalize the current non-committing owner once, release all resource/index state and publish only policy-eligible effects. Immutable ingress attribution survives trusted source promotion: the ban marker is expiring but cannot evict an unexpired revocation fence, post-admission checks remove already queued ingress and the proof-carrying Ready ticket rechecks the same fence before Accepted planning. Banning that ingress therefore removes every matching owner that is still in PrePool in bounded slices, but never rolls back an Accepted transaction whose commit linearized first. A matching Reject releases both relayer known and pending projections so the same valid transaction can be requested from another peer. Clear swaps the complete entry generation and publishes one constant-size GenerationReset, which releases all relayer filters without scanning or retaining a population-sized hash batch. | An already queued, active, Ready or source-promoted malicious-peer transaction must not cross a concurrent ban into Accepted, escape revocation, pin an active slot, retain a budget/index/lease ghost, resurrect through a late lease, remain permanently pinned in the relayer known filter or turn a typed malformed input into fail-stop or an ineligible relay reject. | T1, T2, T3, T4, T7, T8, T10, T11, T13 | - Does administrative removal make every outstanding lease stale atomically?<br>- Are mutable scheduling source and immutable ingress attribution modeled separately, with every derived index and budget cleaned from the authoritative removal plan?<br>- Do both sides of the admission race recheck the same non-evicting ban marker: immediately after new ownership and in the final Ready Plan before Accepted Apply?<br>- Does the cross-crate Reject projection make the same transaction requestable from another peer while leaving Accepted state untouched?<br>- Are ban/reject history and relay policy separate explicit decisions? | Administrative removal uses indexed owners and bounded batches; peer fences are expiry-pruned and share the network ban set's unexpired cardinality without allowing live-fence eviction. |
| `TP-EFFECT-001` Statically partitioned stable-state effects | `tx-pool/src/service/effects.rs`<br>`tx-pool/src/service/builder.rs`<br>`tx-pool/src/callback.rs` | An ordinary mutation-coupled immutable batch passes one exact source-derived region predicate, runs a total Apply and enters the global sequence under the journal's innermost lock; Full is mutation-free and no reservation or capacity token crosses a lock or await. Authority-dependent duplicate success holds an accepted-membership read capability through append. Remote, trusted and chain-critical regions are isolated, resident queued/active records are exactly charged, and only a chain/admin transition that cannot publish detail installs one prebuilt replaceable GenerationReset without waiting behind the FIFO. | Remote saturation, a full critical FIFO, a permanently full relay endpoint, callback/network/database re-entry/failure/hang, accepted duplicate racing clear/reorg, publisher failure, close race or check-before-wait window must not mutate without a matching authority record, consume trusted/critical progress, delay chain convergence, expose intermediate state, reorder a retained batch, lose charge, spawn unbounded workers, fail-stop the service or sleep forever. | T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13 | - Is every ordinary immutable batch exact and assigned from the selected owner's source before Apply?<br>- Does Full/Closed/Oversize return before Apply, with the journal lock innermost and no capacity token crossing a lock or await?<br>- Are Remote, trusted and critical usage accounted independently while queued and active, and does saturated chain detail fall back to the prebuilt reset register rather than wait?<br>- Does every potentially blocking foreign endpoint have a bounded timeout/circuit, and does every authority-dependent acknowledgement retain its read/write capability until journal append? | One bounded journal publisher, one bounded callback endpoint worker and at most one timed-out detached call per blocking endpoint circuit; O(1) admission/accounting, no per-effect retry task, unbounded channel, callback-under-lock or busy retry. |
| `TP-REORG-001` Serialized reliable chain transitions | `tx-pool/src/process/reorg.rs`<br>`tx-pool/src/component/pre_pool/recovery.rs`<br>`tx-pool/src/service.rs`<br>`tx-pool/src/service/pipeline_ops.rs` | One chain-authoritative reorg phase switches snapshot/membership once, combines detached transactions with the complete bounded accepted descendant closure of detached producers, installs one charged parent-first Recovery-source plan in the ordinary six-state kernel, and journals the immediate reset plus level-triggered template generations. | A late accepted child, startup zombie reconcile, over-budget or over-fanout descendant closure, duplicate/colliding detached roots, clear/save races or a stale/failed full-template rebuild must not replay chain mutation, expose a parentless accepted suffix or unowned payload interval, retain transaction-bearing blocks outside the kernel, cross a missing parent prefix or publish stale uncles/template state. | T1, T2, T3, T4, T6, T7, T8, T9, T10, T11, T12, T13 | - Does the paired mutation follow TxPool then PrePoolKernel then innermost EffectJournal, with every recovery payload charged before locks open and no stateful guard crossing an await?<br>- Can any derived retry replay the authoritative chain mutation or retain full detached blocks as a second payload owner?<br>- Are accepted descendants planned before startup zombie reconciliation, retained parent-first, and reset as one ephemeral generation when the complete bounded closure cannot fit?<br>- Are duplicate/collision handling, full/witness identity, non-reused versions and clear epoch ordering exact? | The single graph traversal/transfer stops at fixed mutation and recovery bounds and uses one generation reset beyond them; no recovery lock, replay worker, duplicate accepted traversal or full-block payload owner remains. |
| `TP-PERSIST-001` Coherent persistence recovery point | `tx-pool/src/service.rs`<br>`tx-pool/src/persisted.rs`<br>`tx-pool/src/component/pre_pool/recovery.rs`<br>`tx-pool/src/component/pool_map.rs`<br>`tx-pool/src/process/reorg.rs` | Under one TxPool-read then PrePoolKernel snapshot boundary, persistence copies causal-parent-first accepted entries plus every retained/leased recovery-source payload into a bounded v2 envelope, releases authority locks, and serializes atomic writers without draining live state; startup also accepts the legacy v1 vector and revalidates every transaction. | Save racing detached replay/clear, an active recovery lease, malformed or oversized disk data, writer failure or expanded dep-group ordering must not persist an ownership gap, allocate from an unbounded length, lose the previous atomic file, drain the live pool or serialize children before required parents. | T1, T2, T3, T4, T6, T7, T8, T9, T10, T11, T12, T13 | - Does save use only the universal TxPool-read then kernel order and include accepted xor every recovery owner at one linearization point?<br>- Is PoolMap the accepted ordering authority, including expanded dependencies, while recovery metadata preserves parent-first session order?<br>- Are file bytes/counts bounded before allocation, writes temp-sync-rename atomic, and all startup payloads revalidated rather than trusted? | Shutdown-only clone/sort work stays off admission paths; do not add a continuously maintained persistence projection. |
| `TP-DEFECT-001` Rust-native failure and persistence boundary | `tx-pool/src/component/pre_pool/runtime.rs`<br>`tx-pool/src/component/pre_pool/lifecycle.rs`<br>`tx-pool/src/process/mod.rs`<br>`tx-pool/src/process/submit/mod.rs`<br>`tx-pool/src/service/stages/resolve.rs`<br>`tx-pool/src/service/dispatch.rs`<br>`tx-pool/src/service/stages/verify.rs`<br>`sync/src/relayer/mod.rs` | Static ownership and private constructors make invalid authority states unconstructable; every legal malformed, policy, capacity, duplicate and stale outcome remains typed before Apply; a pre-Apply primary/index/accounting system fault stops without persistence and is never converted to an RPC/peer outcome. Panic plus catch_unwind is not authority, worker, retry or rollback control flow. | A reproducible legal transaction, stale lease, foreign endpoint failure/hang or relay saturation must not panic or reach a structural fault; a genuine pre-Apply system fault must not be converted to success/rejection, restarted into an unknown generation or written to persistence. | T1, T2, T3, T4, T6, T7, T8, T9, T10, T11, T12, T13 | - Can every peer-selectable legal outcome be traced to a typed result before mutation without assert, expect, unwrap, panic or unreachable?<br>- Does the type system prevent mutation between Plan and single-consumption total Apply?<br>- Has panic plus catch_unwind been removed from internal job, worker, publisher and authority protocols?<br>- Are genuinely foreign callbacks/endpoints isolated without selecting tx-pool state, retry or rollback semantics?<br>- Does a pre-Apply system fault stop supervised state work and skip persistence? | Proof-carrying plans and typed results are zero-cost ownership encodings on the healthy path; no recovery mutex, quarantine lookup, spare generation, unwind-driven retry or restart protocol is added. |
| `TP-POOL-001` Atomic accepted-pool graph integrity | `tx-pool/src/component/pool_map.rs`<br>`tx-pool/src/component/links.rs`<br>`tx-pool/src/pool.rs` | Full-hash pool entries are authoritative; proposal slots, status counters, causal links, outpoint indexes, sort keys and aggregate totals are rebuildable projections changed by one immutable bounded plan and total Apply. | Ghost links, short-ID collision, counter drift, late-parent insertion, expired-parent cascades or virtual self-eviction must not corrupt graph weights, preserve impossible children or partially remove accepted entries. | T1, T2, T3, T4, T5, T6, T8, T9, T10, T11, T12, T13 | - Does one immutable plan cover the complete mutation before any write?<br>- Does the independent audit rebuild agree after Apply and remain unchanged after every rejection?<br>- Are required parents distinguished from ordering-only references? | Mutation and audit work is bounded by explicit graph/victim caps; cold repair must never hide hot-path drift. |
| `TP-TEMPLATE-001` Block-template liveness and priority | `tx-pool/src/block_assembler`<br>`tx-pool/src/pool.rs`<br>`tx-pool/src/process/reorg.rs` | Reset and full rebuild retain one ordered publication authority without serializing their construction: a full rebuild derives uncle content from the bounded candidate authority, wins over racing partial revisions but cannot cross a newer reset epoch, while reset consumes its exact generation token. Uncle, proposal and transaction partial updates remain concurrent revision-checked OCC, and every successful full/reset replacement reissues all three optimistic dirty generations so valid recovered transactions always regain a proposal/commit path. | Detached uncle proposals, stale Gap status, an update_full/partial race or stale partial acknowledgement must not make a valid transaction RPC-pending but forever absent from normal get_block_template mining, nor serialize independent partial work behind full calculation. | T3, T4, T5, T6, T8, T9, T10, T11, T12, T13 | - Does the co-located revision/reset epoch preserve full-over-partial priority and reset-over-stale-full authority while re-dirtying uncle/proposal/transaction generations after replacement?<br>- Does a full rebuild derive detached uncles from the candidate authority instead of copying the reset template's transient blank projection?<br>- Can uncle proposal filtering exclude the sole proposal path of a recovered transaction?<br>- Are Gap/Pending/Proposed transitions reflected to assembler selection?<br>- Do uncle/proposal/transaction calculations remain concurrent with a waiting full rebuild and publish only through version checks? | Keep optimistic CAS updates and bounded selection; do not serialize every delta behind full rebuild or remove bounded packing safeguards without measurements. |
| `TP-IDENTITY-001` Full transaction and witness identity | `tx-pool/src/process/submit/mod.rs`<br>`tx-pool/src/component/pre_pool/mod.rs`<br>`tx-pool/src/component/pool_map.rs` | Ownership and duplicate boundaries use full raw hashes, proposal short IDs remain non-authoritative indexes, and verification-cache proofs are keyed only by the exact witness hash through TxVerificationCacheKey. | Short-ID collisions or same-raw/different-witness variants must not alias accepted/cache/history ownership, obtain a false duplicate success or reuse an invalid verification proof during reorg. | T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13 | - Is a short ID used only where collision-aware lookup semantics are explicit?<br>- Can any cache call construct a key from raw hash or arbitrary bytes?<br>- Does reorg recovery query the exact transaction witness variant? | Use compact full hashes/typed cache keys without retaining packed backing; collision handling remains indexed and bounded. |
| `TP-PERF-001` Bounded attacker-controlled work | `tx-pool/src/component/pre_pool/queue.rs`<br>`tx-pool/src/component/pre_pool/commit.rs`<br>`tx-pool/src/benchmark.rs`<br>`tx-pool/scripts/benchmark.py` | Owner-head scheduling, victim selection and conflict probing stop at maintained bounds independent of unrelated population; benchmark comparisons use fingerprinted paired revisions and reject noisy samples. | A capped peer prefix, stronger suffix or large independent population must not turn one admission/checkout into an O(pool) scan; a noisy or mismatched harness must not claim a performance win. | T1, T2, T3, T4, T6, T7, T8, T10, T11, T13 | - Is operation count bounded by owners/cohort/config rather than resident transaction count?<br>- Did the change add allocation, lock, task, scan or mutable projection to a hot path?<br>- Before accepting benchmark evidence, are revision, binary, config, repetitions and spread comparable? | Deterministic operation-count regressions are always required; controlled timing A/B is a separate final gate, never inferred from unit duration. |

### Executable evidence

#### `TP-OWN-001` — Single pre-pool ownership

Minimum command: `cargo nextest run -p ckb-tx-pool --lib -E 'test(/(concrete_kernel_transitions|stale_lease_cannot_mutate|randomized_public_transitions|target_model_generated_commands)/)'`

Rust evidence:

- `concrete_kernel_transitions_preserve_recomputed_projections` (T1, T2, T3, T4, T5, T6, T7, T8, T11)
- `randomized_public_transitions_always_match_full_rebuild` (T1, T2, T3, T4, T5, T6, T7, T8, T10, T11, T13)
- `stale_lease_cannot_mutate_a_removed_and_readmitted_hash` (T1, T2, T3, T4, T7, T11)
- `target_model_declares_exactly_the_frozen_six_states` (T1, T2, T3)
- `target_model_generated_commands_preserve_partition_lease_budget_and_indexes` (T1, T2, T3, T4, T5, T6, T7, T8, T11)
- `target_model_stale_lease_cannot_mutate_a_replaced_witness_owner` (T1, T2, T3, T4, T7, T11)

Process-level evidence:

- `local-test-rpc-direct`: `test/src/specs/tx_pool/local_test_submission.rs::LocalTestSubmissionIsDirect` (T1, T2, T3, T4, T6, T7, T11) — The integration-only send_test_transaction RPC returns a typed missing-parent rejection without stopping the service, then synchronously commits a valid Local transaction with no pre-pool owner or verify-queue residue. Paired units: `local_submit_bypasses_and_settles_matching_remote_owner`. Command: `make integration CKB_TEST_ARGS='-c 1 LocalTestSubmissionIsDirect'`

#### `TP-COMMIT-001` — Authoritative commit and handoff

Minimum command: `cargo nextest run -p ckb-tx-pool --features internal --lib -E 'test(/(pipeline_commit_worker|local_submit_bypasses|target_model_exercises_every_plan_outcome|sparse_plan_matches|accepted_duplicate_relay|resolution_evidence)/)'`

Rust evidence:

- `equal_priority_ready_candidates_commit_earlier_arrival_first` (T1, T2, T4, T6, T7, T11)
- `local_submit_bypasses_and_settles_matching_remote_owner` (T1, T2, T3, T4, T6, T7, T11)
- `pipeline_commit_worker_waits_for_the_pool_sequencer` (T5, T6)
- `target_model_exercises_every_plan_outcome_without_partial_mutation` (T1, T2, T4, T6, T7, T11)
- `tx_pool_resolution_evidence_requires_chain_provenance` (T6, T8)
- `tx_pool_resolution_evidence_yields_to_pool_spends_and_tip_changes` (T6, T9)
- `tx_pool_same_tip_resolution_evidence_skips_chain_revalidation` (T6, T8)

Process-level evidence:

- `local-test-rpc-direct`: `test/src/specs/tx_pool/local_test_submission.rs::LocalTestSubmissionIsDirect` (T1, T2, T3, T4, T6, T7, T11) — The integration-only send_test_transaction RPC returns a typed missing-parent rejection without stopping the service, then synchronously commits a valid Local transaction with no pre-pool owner or verify-queue residue. Paired units: `local_submit_bypasses_and_settles_matching_remote_owner`. Command: `make integration CKB_TEST_ARGS='-c 1 LocalTestSubmissionIsDirect'`

#### `TP-RBF-001` — Deterministic mutation-free RBF planning

Minimum command: `cargo nextest run -p ckb-tx-pool --features internal --lib -E 'test(/(rbf|replacement|self_eviction_plan|failed_size_plan|pipeline_rejects_conflicting_double_spend|full_conflict_history)/)'`

Rust evidence:

- `failed_size_plan_is_mutation_free_with_original_statuses` (T6)
- `permissive_rbf_resolution_cannot_forge_chain_provenance` (T5, T6, T8)
- `pipeline_rejects_conflicting_double_spend` (T5, T6)
- `rbf_rejects_dep_group_member_from_replacement_victim` (T5, T6)
- `rbf_replacement_certain_to_fail_commit_cannot_churn_pool` (T1, T2, T3, T4, T5, T6, T7, T8, T10, T11, T13)
- `self_eviction_plan_leaves_cell_dep_readers_untouched` (T1, T2, T4, T6, T7, T11)
- `successful_replacement_does_not_recover_removed_descendants` (T5, T6)

Process-level evidence:

- `rbf-basic`: `test/src/specs/tx_pool/replace.rs::RbfBasic` (T1, T2, T4, T5, T6, T7, T11) — The node enforces the replacement fee rule, exposes the victim as RBFRejected, commits the accepted higher-fee replacement, and does not reinterpret an output created and consumed in that attached branch as an availability edge that overwrites the root rejection. Paired units: `successful_replacement_does_not_recover_removed_descendants`, `dependency_availability_uses_the_authoritative_overlay_level`. Command: `make integration CKB_TEST_ARGS='-c 1 RbfBasic'`
- `rbf-cell-deps`: `test/src/specs/tx_pool/replace.rs::RbfCellDepsCheck` (T5, T6) — Final node admission rejects a replacement that depends on a cell from its victim closure and records the failed candidate without corrupting the accepted pool graph. Paired units: `rbf_rejects_dep_group_member_from_replacement_victim`. Command: `make integration CKB_TEST_ARGS='-c 1 RbfCellDepsCheck'`
- `rbf-concurrency`: `test/src/specs/tx_pool/replace.rs::RbfConcurrency` (T5, T6) — Concurrent RPC submissions for one input converge to the unique highest-fee transaction and record every losing candidate as rejected without a recovery livelock. Paired units: `pipeline_rejects_conflicting_double_spend`, `successful_replacement_does_not_recover_removed_descendants`. Command: `make integration CKB_TEST_ARGS='-c 1 RbfConcurrency'`
- `rbf-conflict-recovery`: `test/src/specs/tx_pool/orphan_tx_recovery.rs::RbfOrphanRecovery` (T1, T2, T4, T5, T6, T7, T11) — When a replacement frees only one of two historical inputs, the node recovers exactly the newly eligible victim and retains the still-conflicting victim as rejected. Paired units: `full_conflict_history_terminalizes_rejected_owner_without_panicking`, `successful_replacement_does_not_recover_removed_descendants`. Command: `make integration CKB_TEST_ARGS='-c 1 RbfOrphanRecovery'`
- `rbf-cycling-attack`: `test/src/specs/tx_pool/replace.rs::RbfCyclingAttack` (T1, T2, T4, T5, T6, T7, T11) — A cyclic sequence of overlapping replacements recovers the pre-existing transaction whose input becomes free, while the victim retained by that same atomic replacement observes the post-Apply dependency cut and does not resurrect while its input remains consumed. Paired units: `same_commit_wakes_prior_conflict_but_not_its_new_victim`, `full_conflict_history_terminalizes_rejected_owner_without_panicking`, `successful_replacement_does_not_recover_removed_descendants`. Command: `make integration CKB_TEST_ARGS='-c 1 RbfCyclingAttack'`
- `rbf-proposed-success`: `test/src/specs/tx_pool/replace.rs::RbfReplaceProposedSuccess` (T5, T6, T12, T13) — A higher-fee replacement of a Proposed chain member survives a subsequent blank block, is freshly proposed, and is committed while the displaced closure remains rejected. Paired units: `successful_replacement_does_not_recover_removed_descendants`, `commit_and_removal_journal_block_assembler_delta`. Command: `make integration CKB_TEST_ARGS='-c 1 RbfReplaceProposedSuccess'`
- `rbf-proposed-template-refresh`: `test/src/specs/tx_pool/replace.rs::RbfRejectReplaceProposed` (T5, T6, T12, T13) — Replacing a Proposed transaction rejects the victim, refreshes the real node template, and normally proposes and commits only the replacement. Paired units: `successful_replacement_does_not_recover_removed_descendants`, `commit_and_removal_journal_block_assembler_delta`. Command: `make integration CKB_TEST_ARGS='-c 1 RbfRejectReplaceProposed'`

#### `TP-DEP-001` — Causal dependency graph

Minimum command: `cargo nextest run -p ckb-tx-pool --features internal --lib -E 'test(/(parent_commit_before_wait|parent_loss|successive_expanded_parent_losses|terminalized_superseded_parent|dependency_epochs|dep_reader|selected_reader|conditional_cycle|dep_group|unknown_outpoint)/)'`

Rust evidence:

- `conditional_cycle_does_not_drop_acyclic_downstream_entry` (T3, T4, T8, T10, T11, T12, T13)
- `conditional_cycle_drops_weakest_member` (T3, T4, T8, T10, T11, T12, T13)
- `definitive_parent_terminalization_wakes_trusted_dependents` (T1, T2, T3, T4, T7, T8, T10, T11, T13)
- `dense_conditional_scc_uses_bounded_fallback_and_keeps_strongest` (T3, T4, T8, T10, T11, T12, T13)
- `local_rbf_commit_demotes_consumer_of_live_expanded_dep_group_member` (T4, T11)
- `over_budget_dep_entry_does_not_censor_independent_suffix` (T3, T4, T8, T10, T11, T12, T13)
- `parent_commit_before_wait_registration_requeues_child` (T1, T2, T4, T7, T11)
- `parent_loss_invalidates_an_active_lease_into_exact_wait` (T1, T2, T4, T7, T11)
- `pipeline_accepts_dep_reader_after_in_flight_spender` (T1, T2, T4, T6, T7, T11)
- `remote_parent_wait_and_unknown_parents_effect_are_one_transition` (T1, T2, T4, T7, T11)
- `repeated_dependency_epochs_are_level_triggered_and_bounded` (T1, T2, T3, T4, T7, T8, T10, T11, T13)
- `resolve_job_registers_complete_unknown_outpoint_frontier` (T1, T2, T4, T7, T11)
- `selected_reader_is_ordered_before_spender` (T4, T11, T12, T13)
- `sort_txs_by_dependencies_orders_parents_before_children` (T4, T11)
- `successive_expanded_parent_losses_keep_exact_causal_keys_in_wait` (T1, T2, T4, T7, T11)
- `target_model_wait_wake_and_ready_conflict_use_recomputed_views` (T4, T5, T6, T11)
- `terminalized_superseded_parent_wakes_its_trusted_child` (T1, T2, T4, T7, T11, T12, T13)

Process-level evidence:

- `accepted-descendants-after-parent-reorg`: `test/src/specs/tx_pool/pool_reconcile.rs::PoolResolveConflictAfterReorg` (T1, T2, T3, T4, T6, T9, T10, T11, T12, T13) — When a formerly committed parent is detached while its child and grandchild remain accepted, the node transfers the complete accepted closure into parent-first recovery, restores all three as Proposed and retains the ordinary dead-input conflict verdict. Paired units: `accepted_reorg_recovery_plan_is_parent_first_and_total`, `reorg_replays_detached_parent_with_accepted_descendant_closure`, `over_bound_reorg_descendant_closure_resets_ephemeral_pool_generation`. Command: `make integration CKB_TEST_ARGS='-c 1 PoolResolveConflictAfterReorg'`
- `cell-dep-arrival-order`: `test/src/specs/tx_pool/dead_cell_deps.rs::CellBeingCellDepAndSpentInSameBlockTestGetBlockTemplate` (T4, T6, T11, T12, T13) — Both reader-first and spender-first RPC arrival orders retain the valid pair; normal get_block_template deterministically places the cell-dep reader before the spender and commits both. Paired units: `pipeline_accepts_dep_reader_after_in_flight_spender`, `selected_reader_is_ordered_before_spender`, `conditional_cycle_drops_weakest_member`. Command: `make integration CKB_TEST_ARGS='-c 1 CellBeingCellDepAndSpentInSameBlockTestGetBlockTemplate'`
- `conditional-reader-fanout`: `test/src/specs/tx_pool/limit.rs::TxPoolLimitAncestorCount` (T3, T6, T8, T10, T11, T13) — Two thousand cell-dep readers coexist with a spender without becoming persistent ancestors, while a genuine causal chain still stops at the configured ancestor limit. Paired units: `popular_dep_readers_coexist_with_spender`, `dep_readers_do_not_count_as_spender_ancestors`, `causal_ancestor_limit_never_evicts_existing_entries`. Command: `make integration CKB_TEST_ARGS='-c 1 TxPoolLimitAncestorCount'`
- `dependent-reorg-chain`: `test/src/specs/tx_pool/reorg_recovers_dependent.rs::ReorgRecoversDependentChain` (T4, T9, T10, T11, T12, T13) — A three-level detached dependency chain is replayed parent-first and every member returns to a committable pool state after the node adopts the competing fork. Paired units: `sort_txs_by_dependencies_orders_parents_before_children`, `reorg_direct_replay_treats_pool_duplicates_as_idempotent`. Command: `make integration CKB_TEST_ARGS='-c 1 ReorgRecoversDependentChain'`
- `dependent-reorg-transactions`: `test/src/specs/tx_pool/reorg_recovers_dependent.rs::ReorgRecoversDependentTxs` (T4, T9, T10, T11, T12, T13) — A detached parent and its dependent child are both recovered through the node reorg callback and remain committable as one dependency-ordered pool graph. Paired units: `sort_txs_by_dependencies_orders_parents_before_children`, `reorg_direct_replay_treats_pool_duplicates_as_idempotent`. Command: `make integration CKB_TEST_ARGS='-c 1 ReorgRecoversDependentTxs'`
- `multi-parent-orphan-frontier`: `test/src/specs/tx_pool/orphan_tx.rs::TxPoolOrphanReverse` (T1, T2, T4, T7, T10, T11) — A child with three unavailable direct parents installs one exact Wait owner and atomically requests the complete currently missing parent frontier, so RelayV3 accepts every reverse-order parent and the whole graph converges without polling or an inferred orphan store. Paired units: `resolve_job_registers_complete_unknown_outpoint_frontier`, `remote_parent_wait_and_unknown_parents_effect_are_one_transition`, `journal_sequence_is_total_apply_order`. Command: `make integration CKB_TEST_ARGS='-c 1 TxPoolOrphanReverse'`

#### `TP-CACHE-001` — Conflict-history Wait ownership and wakeup

Minimum command: `cargo nextest run -p ckb-tx-pool --lib -E 'test(/(full_conflict_history|same_commit_wakes_prior_conflict|repeated_dependency_epochs|availability_without_a_wait|dependency_availability_uses_the_authoritative_overlay|remote_conflict_history|remote_peer_order|pipeline_rejects_conflicting_double_spend)/)'`

Rust evidence:

- `availability_without_a_wait_owner_retains_no_epoch_history` (T3, T8, T10, T11, T13)
- `dependency_availability_uses_the_authoritative_overlay_level` (T1, T2, T4, T5, T6, T7, T11)
- `full_conflict_history_terminalizes_rejected_owner_without_panicking` (T1, T2, T3, T4, T5, T6, T7, T8, T11)
- `remote_conflict_history_keeps_its_bounded_residency_deadline` (T1, T2, T3, T4, T7, T8, T11)
- `remote_peer_order_cannot_hijack_an_existing_conflict_owner` (T1, T2, T3, T5, T6)
- `same_commit_wakes_prior_conflict_but_not_its_new_victim` (T1, T2, T4, T5, T6, T7, T11)

Process-level evidence:

- `rbf-basic`: `test/src/specs/tx_pool/replace.rs::RbfBasic` (T1, T2, T4, T5, T6, T7, T11) — The node enforces the replacement fee rule, exposes the victim as RBFRejected, commits the accepted higher-fee replacement, and does not reinterpret an output created and consumed in that attached branch as an availability edge that overwrites the root rejection. Paired units: `successful_replacement_does_not_recover_removed_descendants`, `dependency_availability_uses_the_authoritative_overlay_level`. Command: `make integration CKB_TEST_ARGS='-c 1 RbfBasic'`
- `rbf-conflict-recovery`: `test/src/specs/tx_pool/orphan_tx_recovery.rs::RbfOrphanRecovery` (T1, T2, T4, T5, T6, T7, T11) — When a replacement frees only one of two historical inputs, the node recovers exactly the newly eligible victim and retains the still-conflicting victim as rejected. Paired units: `full_conflict_history_terminalizes_rejected_owner_without_panicking`, `successful_replacement_does_not_recover_removed_descendants`. Command: `make integration CKB_TEST_ARGS='-c 1 RbfOrphanRecovery'`
- `rbf-cycling-attack`: `test/src/specs/tx_pool/replace.rs::RbfCyclingAttack` (T1, T2, T4, T5, T6, T7, T11) — A cyclic sequence of overlapping replacements recovers the pre-existing transaction whose input becomes free, while the victim retained by that same atomic replacement observes the post-Apply dependency cut and does not resurrect while its input remains consumed. Paired units: `same_commit_wakes_prior_conflict_but_not_its_new_victim`, `full_conflict_history_terminalizes_rejected_owner_without_panicking`, `successful_replacement_does_not_recover_removed_descendants`. Command: `make integration CKB_TEST_ARGS='-c 1 RbfCyclingAttack'`

#### `TP-BUDGET-001` — Continuous hostile-state accounting

Minimum command: `cargo nextest run -p ckb-tx-pool --features internal --lib -E 'test(/(admission_budget_failure|parent_loss_uses_the_continuous_wait_reservation|parent_projection_deduplicates|remote_conflict_keeps_remote_reservation|owner_fairness_and_active_caps|verified_candidate_compacts|conflict_closure_aborts|self_eviction_plan|multi_input_conflict_union_respects_the_global_commit_bound)/)'`

Rust evidence:

- `admission_budget_failure_leaves_primary_and_views_unchanged` (T1, T2, T3, T4, T7, T8, T11)
- `conflict_closure_aborts_at_candidate_limit` (T3, T8, T10, T11, T13)
- `journal_usage_charges_queued_and_active_batches_exactly` (T3, T8)
- `multi_input_conflict_union_respects_the_global_commit_bound` (T3, T8, T10, T11, T13)
- `parent_loss_uses_the_continuous_wait_reservation_at_a_full_budget` (T1, T2, T3, T4, T7, T8, T11)
- `parent_projection_deduplicates_cell_and_header_edges` (T3, T6, T8)
- `remote_conflict_keeps_remote_reservation_and_wakes_without_capacity_retry` (T1, T2, T3, T4, T7, T8, T10, T11, T13)
- `verified_candidate_compacts_deps_and_pool_budget_counts_retained_inputs` (T3, T8)

#### `TP-WORKER-001` — Level-triggered executable readiness

Minimum command: `cargo nextest run -p ckb-tx-pool --lib -E 'test(/(owner_fairness_and_active_caps|expiry_batch_is_bounded|worker_|zero_verify_worker)/)'`

Rust evidence:

- `expiry_batch_is_bounded_without_a_ready_prefix_scan` (T1, T2, T3, T4, T7, T8, T10, T11, T13)
- `large_owner_head_does_not_hide_its_small_cycle_work` (T1, T2, T3, T4, T7, T8, T10, T11, T13)
- `panicked_state_worker_makes_shutdown_ineligible_for_persistence` (T1, T2, T3, T4, T7, T8, T10, T11, T13)
- `service_cancellation_is_scoped_under_process_exit` (T1, T4, T7)
- `worker_exits_when_command_channel_dropped` (T1, T2, T3, T4, T7, T8, T10, T11, T13)
- `zero_verify_worker_config_still_runs_remote_pipeline` (T1, T2, T4, T7, T11)

#### `TP-ADMIN-001` — Administrative and hostile-peer terminalization

Minimum command: `cargo nextest run -p ckb-tx-pool --features internal -E 'test(/(banned_peer_fence_never_evicts_an_unexpired_marker|banned_peer_revokes|banned_peer_revocation_plan_uses_immutable_ingress_attribution|peer_ban_removes_promoted_ingress_and_allows_refetch|peer_ban_does_not_rollback_an_already_accepted_transaction|ready_commit_observes_ban_fence_before_acceptance|queued_remote_admission_after_ban_is_removed_and_refetchable|proposal_promotes_active_remote_owner_without_restarting_lease|malformed_remote_preflight|proposal_promoted_remote_clear)/)'`

Rust evidence:

- `banned_peer_fence_never_evicts_an_unexpired_marker` (T1, T2, T3, T8)
- `banned_peer_revocation_plan_uses_immutable_ingress_attribution` (T1, T2, T3, T4, T7, T8, T11)
- `banned_peer_revokes_active_remote_lease_and_releases_budget` (T1, T2, T3, T4, T7, T8, T11)
- `malformed_remote_preflight_is_banned_recorded_and_not_relayed` (T1, T2, T4, T7, T10, T11)
- `peer_ban_does_not_rollback_an_already_accepted_transaction` (T1, T2, T3, T4, T7, T10, T11)
- `peer_ban_removes_promoted_ingress_and_allows_refetch` (T1, T2, T3, T4, T7, T8, T10, T11)
- `proposal_promoted_remote_clear_uses_generation_reset_to_release_ingress_filter` (T1, T2, T3, T4, T7, T8, T10, T11, T13)
- `proposal_promotes_active_remote_owner_without_restarting_lease` (T1, T2, T3, T4, T7, T8, T11)
- `queued_remote_admission_after_ban_is_removed_and_refetchable` (T1, T2, T3, T4, T7, T8, T10, T11)
- `ready_commit_observes_ban_fence_before_acceptance` (T1, T2, T3, T4, T7, T8, T10, T11)

#### `TP-EFFECT-001` — Statically partitioned stable-state effects

Minimum command: `cargo nextest run -p ckb-tx-pool --features internal --lib -E 'test(/(journal_usage|journal_sequence|full_journal|ordinary_full|remote_byte_ceiling|proposal_ready_commit|critical_headroom|authoritative_apply|authoritative_generation_reset|closed_journal_rejects_generation_reset|generation_reset_register|full_relayer|hung_callback|hung_network_endpoint|accepted_duplicate_relay|replacement_publisher|close_wakes|idle_publisher)/)'`

Rust evidence:

- `accepted_duplicate_relay_cannot_overtake_a_waiting_clear_reset` (T1, T2, T3, T5, T6, T7, T10)
- `active_critical_batch_does_not_consume_ordinary_headroom` (T3, T7, T8, T10, T11, T13)
- `authoritative_apply_falls_back_to_prebuilt_reset_when_fifo_is_full` (T3, T7, T8, T9, T10, T11, T12, T13)
- `authoritative_generation_reset_is_explicit_even_with_fifo_capacity` (T7, T9, T10, T12, T13)
- `close_wakes_every_blocked_capacity_waiter` (T3, T7, T8, T10, T11, T13)
- `closed_journal_rejects_generation_reset_before_authority_apply` (T7, T9, T10, T12, T13)
- `full_journal_does_not_run_the_state_apply_closure` (T3, T7, T8, T10)
- `full_relayer_coalesces_to_bounded_reconciliation` (T3, T7, T8, T10, T11, T13)
- `hung_callback_opens_one_stable_circuit_and_does_not_pin_relay` (T3, T7, T8, T10, T11, T13)
- `hung_network_endpoint_opens_one_stable_circuit_and_does_not_pin_relay` (T3, T7, T8, T10, T11, T13)
- `idle_publisher_observes_close_without_a_later_ready_event` (T3, T7, T8, T10, T11, T13)
- `journal_sequence_is_total_apply_order` (T7, T10)
- `ordinary_full_is_mutation_free_and_does_not_install_reset` (T3, T7, T8, T10)
- `proposal_ready_commit_uses_trusted_effect_headroom` (T3, T7, T8, T10, T11, T13)
- `remote_byte_ceiling_cannot_borrow_trusted_headroom` (T3, T7, T8, T10, T11, T13)
- `replacement_publisher_resumes_the_charged_active_batch` (T3, T7, T8, T10)
- `trusted_saturation_cannot_consume_critical_headroom` (T3, T7, T8, T10, T11, T13)

Process-level evidence:

- `multi-parent-orphan-frontier`: `test/src/specs/tx_pool/orphan_tx.rs::TxPoolOrphanReverse` (T1, T2, T4, T7, T10, T11) — A child with three unavailable direct parents installs one exact Wait owner and atomically requests the complete currently missing parent frontier, so RelayV3 accepts every reverse-order parent and the whole graph converges without polling or an inferred orphan store. Paired units: `resolve_job_registers_complete_unknown_outpoint_frontier`, `remote_parent_wait_and_unknown_parents_effect_are_one_transition`, `journal_sequence_is_total_apply_order`. Command: `make integration CKB_TEST_ARGS='-c 1 TxPoolOrphanReverse'`

#### `TP-REORG-001` — Serialized reliable chain transitions

Minimum command: `cargo nextest run -p ckb-tx-pool --features internal -E 'test(/(reorg_publishes|overlapping_detached|accepted_reorg_recovery|reorg_replays_detached_parent|over_bound_reorg|recovery_batch|empty_generation_recovery|failed_block_assembler|clear_during_reorg|cross_authority_query)/)'`

Rust evidence:

- `accepted_reorg_recovery_plan_is_parent_first_and_total` (T1, T2, T3, T6, T9, T10, T12, T13)
- `accepted_reorg_recovery_plan_reports_over_bound_fanout` (T3, T8, T9, T10, T11, T12, T13)
- `clear_during_reorg_recovery_owns_the_final_empty_state` (T1, T2, T4, T7, T9, T10, T11, T12, T13)
- `cross_authority_query_is_serialized_with_clear_and_reorg` (T1, T2, T3, T6, T9, T10, T12, T13)
- `empty_generation_recovery_retains_closure_safe_prefix` (T3, T4, T8, T9, T10, T11, T12, T13)
- `failed_block_assembler_update_retains_dirty_generation_for_retry` (T9, T10, T12, T13)
- `over_bound_reorg_descendant_closure_resets_ephemeral_pool_generation` (T1, T2, T3, T4, T7, T8, T9, T10, T11, T12, T13)
- `over_budget_recovery_plan_is_mutation_free` (T1, T2, T3, T4, T7, T8, T9, T10, T11, T12, T13)
- `overlapping_detached_proposals_requeue_each_descendant_once` (T9, T10, T12, T13)
- `recovery_batch_is_atomic_parent_first_and_uses_ordered_resolve` (T1, T2, T3, T8, T9, T10, T12, T13)
- `reorg_direct_replay_treats_pool_duplicates_as_idempotent` (T1, T2, T4, T7, T9, T10, T11, T12, T13)
- `reorg_publishes_only_the_final_status_after_multiple_transitions` (T9, T10, T12, T13)
- `reorg_replays_detached_parent_with_accepted_descendant_closure` (T1, T2, T3, T4, T6, T9, T10, T11, T12, T13)

Process-level evidence:

- `accepted-descendants-after-parent-reorg`: `test/src/specs/tx_pool/pool_reconcile.rs::PoolResolveConflictAfterReorg` (T1, T2, T3, T4, T6, T9, T10, T11, T12, T13) — When a formerly committed parent is detached while its child and grandchild remain accepted, the node transfers the complete accepted closure into parent-first recovery, restores all three as Proposed and retains the ordinary dead-input conflict verdict. Paired units: `accepted_reorg_recovery_plan_is_parent_first_and_total`, `reorg_replays_detached_parent_with_accepted_descendant_closure`, `over_bound_reorg_descendant_closure_resets_ephemeral_pool_generation`. Command: `make integration CKB_TEST_ARGS='-c 1 PoolResolveConflictAfterReorg'`
- `async-uncle-candidate-publication`: `test/src/specs/mining/uncle.rs::UncleInheritFromForkUncle` (T9, T10, T12, T13) — After a forced fork, the process test waits for phase-two detached-uncle publication instead of mistaking the authoritative blank reset template for final convergence; it then consumes every eligible uncle and validates the descendant rule. Paired units: `failed_block_assembler_update_retains_dirty_generation_for_retry`, `full_reset_and_partial_priority_use_template_owned_tokens`, `full_rebuild_derives_uncles_from_candidate_authority`, `reorg_refresh_recovers_when_blank_reset_precedes_candidate_retention`, `full_rebuild_reissues_every_optimistic_delta_generation`. Command: `make integration CKB_TEST_ARGS='-c 1 UncleInheritFromForkUncle'`
- `dependent-reorg-chain`: `test/src/specs/tx_pool/reorg_recovers_dependent.rs::ReorgRecoversDependentChain` (T4, T9, T10, T11, T12, T13) — A three-level detached dependency chain is replayed parent-first and every member returns to a committable pool state after the node adopts the competing fork. Paired units: `sort_txs_by_dependencies_orders_parents_before_children`, `reorg_direct_replay_treats_pool_duplicates_as_idempotent`. Command: `make integration CKB_TEST_ARGS='-c 1 ReorgRecoversDependentChain'`
- `dependent-reorg-transactions`: `test/src/specs/tx_pool/reorg_recovers_dependent.rs::ReorgRecoversDependentTxs` (T4, T9, T10, T11, T12, T13) — A detached parent and its dependent child are both recovered through the node reorg callback and remain committable as one dependency-ordered pool graph. Paired units: `sort_txs_by_dependencies_orders_parents_before_children`, `reorg_direct_replay_treats_pool_duplicates_as_idempotent`. Command: `make integration CKB_TEST_ARGS='-c 1 ReorgRecoversDependentTxs'`
- `normal-mining-reorg-tree`: `test/src/specs/tx_pool/reorg_recovers_dependent.rs::ReorgRecoversDependentPendingTree` (T9, T10, T12, T13) — A detached dependent tree is recovered as real Pending and is re-proposed and committed by normal get_block_template mining without relying on optional uncles or a hand-authored proposal block. Paired units: `reorg_publishes_only_the_final_status_after_multiple_transitions`, `reorg_demotes_stale_gap_to_pending`, `pending_proposals_filter_conflicting_uncle_subtree`. Command: `make integration CKB_TEST_ARGS='-c 1 ReorgRecoversDependentPendingTree'`

#### `TP-PERSIST-001` — Coherent persistence recovery point

Minimum command: `cargo nextest run -p ckb-tx-pool --features internal -E 'test(/(save_pool_captures_complete_reorg_ownership|dependent_chain_survives_save_and_restart|dispatcher_channel_close|persisted_file_orders|persistence_v2_rejects_oversized|persistence_loader_accepts_legacy)/)'`

Rust evidence:

- `dependent_chain_survives_save_and_restart` (T4, T9, T10, T11, T12, T13)
- `dispatcher_channel_close_quiesces_workers_and_persists_pool` (T7, T9, T10, T12, T13)
- `persisted_file_orders_expanded_dep_group_parents` (T6, T9, T10, T12, T13)
- `persistence_loader_accepts_legacy_v1_vector` (T9, T10, T12, T13)
- `persistence_v2_rejects_oversized_file_before_reading_payload` (T3, T8, T9, T10, T12, T13)
- `save_pool_captures_complete_reorg_ownership` (T1, T2, T3, T9, T10, T12, T13)

#### `TP-DEFECT-001` — Rust-native failure and persistence boundary

Minimum command: `cargo nextest run -p ckb-tx-pool --features internal --lib -E 'test(/(journal_accounting_drift|authoritative_generation_swap|chain_generation_reset|panicked_effect_publisher|phase_capacity_growth_is_public_rejection|structural_fault_is_not_a_transaction_or_peer_rejection|system_fault_is_not_transaction_policy)/)'`

Rust evidence:

- `authoritative_generation_swap_preserves_aba_clocks` (T1, T2, T3, T4, T7, T9, T10, T11, T12, T13)
- `chain_generation_reset_retires_old_generation_outside_the_lock` (T1, T2, T3, T4, T7, T8, T9, T10, T11, T12, T13)
- `direct_submission_system_fault_is_not_transaction_policy` (T1, T2, T4, T7, T9, T10, T11, T12, T13)
- `generation_reset_register_bypasses_full_fifo_and_coalesces` (T1, T2, T3, T4, T7, T8, T10, T11)
- `ingress_structural_fault_is_not_a_transaction_or_peer_rejection` (T1, T2, T4, T7, T9, T10, T11, T12, T13)
- `journal_accounting_drift_returns_typed_fault_without_partial_completion` (T1, T2, T3, T4, T7, T8, T10, T11, T13)
- `panicked_effect_publisher_makes_shutdown_ineligible_for_persistence` (T1, T2, T4, T7, T9, T10, T11, T12, T13)
- `resolve_phase_capacity_growth_is_public_rejection` (T1, T2, T3, T4, T7, T8, T10, T11)
- `verify_phase_capacity_growth_is_public_rejection` (T1, T2, T3, T4, T7, T8, T10, T11)

Process-level evidence:

- `local-test-rpc-direct`: `test/src/specs/tx_pool/local_test_submission.rs::LocalTestSubmissionIsDirect` (T1, T2, T3, T4, T6, T7, T11) — The integration-only send_test_transaction RPC returns a typed missing-parent rejection without stopping the service, then synchronously commits a valid Local transaction with no pre-pool owner or verify-queue residue. Paired units: `local_submit_bypasses_and_settles_matching_remote_owner`. Command: `make integration CKB_TEST_ARGS='-c 1 LocalTestSubmissionIsDirect'`

#### `TP-POOL-001` — Atomic accepted-pool graph integrity

Minimum command: `cargo nextest run -p ckb-tx-pool --features internal --lib -E 'test(/(sparse_plan_matches|conflict_closure_ignores|status_counter_underflow|causal_ancestor_limit|popular_dep_readers|test_dep_group|parent_added_after_child|reorg_expiry_cascades)/)'`

Rust evidence:

- `causal_ancestor_limit_never_evicts_existing_entries` (T6)
- `conflict_closure_ignores_ghost_link_nodes` (T6)
- `dep_readers_do_not_count_as_spender_ancestors` (T6)
- `parent_added_after_child_gets_descendant_weight` (T6)
- `popular_dep_readers_coexist_with_spender` (T3, T6, T8, T10, T11, T13)
- `reorg_expiry_cascades_from_expired_parent_to_fresh_child` (T6)
- `sparse_plan_matches_stepwise_reference_across_small_graphs` (T3, T6, T8, T10, T11, T13)
- `status_counter_underflow_returns_typed_fault_without_partial_removal` (T6)
- `test_dep_group` (T6)

Process-level evidence:

- `accepted-descendants-after-parent-reorg`: `test/src/specs/tx_pool/pool_reconcile.rs::PoolResolveConflictAfterReorg` (T1, T2, T3, T4, T6, T9, T10, T11, T12, T13) — When a formerly committed parent is detached while its child and grandchild remain accepted, the node transfers the complete accepted closure into parent-first recovery, restores all three as Proposed and retains the ordinary dead-input conflict verdict. Paired units: `accepted_reorg_recovery_plan_is_parent_first_and_total`, `reorg_replays_detached_parent_with_accepted_descendant_closure`, `over_bound_reorg_descendant_closure_resets_ephemeral_pool_generation`. Command: `make integration CKB_TEST_ARGS='-c 1 PoolResolveConflictAfterReorg'`
- `conditional-reader-fanout`: `test/src/specs/tx_pool/limit.rs::TxPoolLimitAncestorCount` (T3, T6, T8, T10, T11, T13) — Two thousand cell-dep readers coexist with a spender without becoming persistent ancestors, while a genuine causal chain still stops at the configured ancestor limit. Paired units: `popular_dep_readers_coexist_with_spender`, `dep_readers_do_not_count_as_spender_ancestors`, `causal_ancestor_limit_never_evicts_existing_entries`. Command: `make integration CKB_TEST_ARGS='-c 1 TxPoolLimitAncestorCount'`
- `rbf-cell-deps`: `test/src/specs/tx_pool/replace.rs::RbfCellDepsCheck` (T5, T6) — Final node admission rejects a replacement that depends on a cell from its victim closure and records the failed candidate without corrupting the accepted pool graph. Paired units: `rbf_rejects_dep_group_member_from_replacement_victim`. Command: `make integration CKB_TEST_ARGS='-c 1 RbfCellDepsCheck'`

#### `TP-TEMPLATE-001` — Block-template liveness and priority

Minimum command: `cargo nextest run -p ckb-tx-pool --features internal --lib -E 'test(/(reorg_demotes_stale_gap|pending_proposals_filter|full_reset_and_partial_priority_use_template_owned_tokens|full_rebuild_derives_uncles_from_candidate_authority|reorg_refresh_recovers_when_blank_reset_precedes_candidate_retention|full_rebuild_reissues_every_optimistic_delta|selected_reader|conditional_cycle)/)'`

Rust evidence:

- `commit_and_removal_journal_block_assembler_delta` (T12, T13)
- `full_rebuild_derives_uncles_from_candidate_authority` (T3, T8, T9, T10, T11, T12, T13)
- `full_rebuild_reissues_every_optimistic_delta_generation` (T9, T10, T12, T13)
- `full_reset_and_partial_priority_use_template_owned_tokens` (T3, T8, T9, T10, T11, T12, T13)
- `pending_proposals_filter_conflicting_uncle_subtree` (T12, T13)
- `reorg_demotes_stale_gap_to_pending` (T12, T13)
- `reorg_refresh_recovers_when_blank_reset_precedes_candidate_retention` (T3, T8, T9, T10, T11, T12, T13)

Process-level evidence:

- `async-uncle-candidate-publication`: `test/src/specs/mining/uncle.rs::UncleInheritFromForkUncle` (T9, T10, T12, T13) — After a forced fork, the process test waits for phase-two detached-uncle publication instead of mistaking the authoritative blank reset template for final convergence; it then consumes every eligible uncle and validates the descendant rule. Paired units: `failed_block_assembler_update_retains_dirty_generation_for_retry`, `full_reset_and_partial_priority_use_template_owned_tokens`, `full_rebuild_derives_uncles_from_candidate_authority`, `reorg_refresh_recovers_when_blank_reset_precedes_candidate_retention`, `full_rebuild_reissues_every_optimistic_delta_generation`. Command: `make integration CKB_TEST_ARGS='-c 1 UncleInheritFromForkUncle'`
- `cell-dep-arrival-order`: `test/src/specs/tx_pool/dead_cell_deps.rs::CellBeingCellDepAndSpentInSameBlockTestGetBlockTemplate` (T4, T6, T11, T12, T13) — Both reader-first and spender-first RPC arrival orders retain the valid pair; normal get_block_template deterministically places the cell-dep reader before the spender and commits both. Paired units: `pipeline_accepts_dep_reader_after_in_flight_spender`, `selected_reader_is_ordered_before_spender`, `conditional_cycle_drops_weakest_member`. Command: `make integration CKB_TEST_ARGS='-c 1 CellBeingCellDepAndSpentInSameBlockTestGetBlockTemplate'`
- `normal-mining-reorg-tree`: `test/src/specs/tx_pool/reorg_recovers_dependent.rs::ReorgRecoversDependentPendingTree` (T9, T10, T12, T13) — A detached dependent tree is recovered as real Pending and is re-proposed and committed by normal get_block_template mining without relying on optional uncles or a hand-authored proposal block. Paired units: `reorg_publishes_only_the_final_status_after_multiple_transitions`, `reorg_demotes_stale_gap_to_pending`, `pending_proposals_filter_conflicting_uncle_subtree`. Command: `make integration CKB_TEST_ARGS='-c 1 ReorgRecoversDependentPendingTree'`
- `rbf-proposed-success`: `test/src/specs/tx_pool/replace.rs::RbfReplaceProposedSuccess` (T5, T6, T12, T13) — A higher-fee replacement of a Proposed chain member survives a subsequent blank block, is freshly proposed, and is committed while the displaced closure remains rejected. Paired units: `successful_replacement_does_not_recover_removed_descendants`, `commit_and_removal_journal_block_assembler_delta`. Command: `make integration CKB_TEST_ARGS='-c 1 RbfReplaceProposedSuccess'`
- `rbf-proposed-template-refresh`: `test/src/specs/tx_pool/replace.rs::RbfRejectReplaceProposed` (T5, T6, T12, T13) — Replacing a Proposed transaction rejects the victim, refreshes the real node template, and normally proposes and commits only the replacement. Paired units: `successful_replacement_does_not_recover_removed_descendants`, `commit_and_removal_journal_block_assembler_delta`. Command: `make integration CKB_TEST_ARGS='-c 1 RbfRejectReplaceProposed'`

#### `TP-IDENTITY-001` — Full transaction and witness identity

Minimum command: `cargo nextest run -p ckb-tx-pool --lib -E 'test(/(short_id_collision|full_hash_lookup|proposal_witness_variant|trusted_conflict_resubmission|verification_cache_isolated|reorg_recovery_reads_cache)/)'`

Rust evidence:

- `full_hash_lookup_does_not_alias_a_proposal_short_id_collision` (T1, T2, T3, T6)
- `pool_short_id_collision_is_not_a_successful_duplicate` (T1, T2, T3, T6)
- `proposal_witness_variant_replaces_remote_payload_at_authoritative_handoff` (T1, T2, T3, T4, T7, T8, T11)
- `reorg_recovery_reads_cache_by_exact_witness_hash` (T1, T2, T3, T5, T6, T9, T10, T12, T13)
- `short_id_collision_is_backpressure_not_aliasing` (T1, T2, T3, T8)
- `synchronous_precheck_does_not_alias_short_id_collision_as_duplicate` (T1, T2, T3, T4, T7, T11)
- `trusted_conflict_resubmission_refreshes_the_exact_witness_owner` (T1, T2, T3, T5, T6)
- `verification_cache_isolated_by_witness_hash_not_raw_hash` (T1, T2, T3, T5, T6)

#### `TP-PERF-001` — Bounded attacker-controlled work

Minimum command: `cargo nextest run -p ckb-tx-pool --lib -E 'test(/(owner_fairness_and_active_caps|expiry_batch_is_bounded|randomized_public_transitions)/)'`

Rust evidence:

- `controller_dependent_secp_chain_reverse` (T1, T2, T3, T4, T7, T8, T10, T11, T13)
- `descendants_cache_members_stay_within_budget` (T3, T8, T10, T11, T13)
- `owner_fairness_and_active_caps_do_not_scan_a_capped_prefix` (T3, T8, T10, T11, T13)

Process-level evidence:

- `conditional-reader-fanout`: `test/src/specs/tx_pool/limit.rs::TxPoolLimitAncestorCount` (T3, T6, T8, T10, T11, T13) — Two thousand cell-dep readers coexist with a spender without becoming persistent ancestors, while a genuine causal chain still stops at the configured ancestor limit. Paired units: `popular_dep_readers_coexist_with_spender`, `dep_readers_do_not_count_as_spender_ancestors`, `causal_ancestor_limit_never_evicts_existing_entries`. Command: `make integration CKB_TEST_ARGS='-c 1 TxPoolLimitAncestorCount'`

<!-- END GENERATED: TX_POOL_BEHAVIORS -->

## Source and test navigation contract

The directory tree mirrors the authority boundary so a reviewer can move from
behavior to implementation without reconstructing historical module names:

| Responsibility | Production location | Behavioral tests |
|---|---|---|
| accepted membership and immutable mutation plans | `src/pool.rs`, `src/component/pool_map.rs` | `src/component/tests/` |
| pre-accepted six-state owner | `src/component/pre_pool/` | `src/component/tests/pre_pool_*` |
| cross-authority service operations | `src/service/pipeline_ops.rs` | `src/service/tests/` |
| asynchronous resolve/verify execution | `src/service/stages/` | `src/service/tests/pipeline/`, `src/service/tests/stage_runner.rs` |
| stable effects and publication | `src/service/effects.rs` | `src/service/tests/effects.rs` |
| template construction and optimistic publication | `src/block_assembler/` | `src/block_assembler/tests/` |

Layout rules are mechanical review guarantees:

- `tests/mod.rs` is the sole root when a module has a `tests/` directory; do
  not add a neighboring `tests.rs`.
- Behavior-bearing files use domain names such as `persistence.rs` or
  `pre_pool_queue.rs`. Helper-only private bridges end in `_test_support.rs`;
  the historical `_seam.rs` name is retired.
- Service-level scenarios and their harness live under `service/tests`, not
  under `component/tests`. Reusable transaction fixtures live in the crate
  `#[cfg(test)]` module `test_support` and do not widen production APIs.
- Production files contain only explicit `cfg(test)` module wiring or reviewed
  observation seams. Every wire and seam is frozen by
  [`test-layout-manifest.json`](../test-layout-manifest.json).
- Renaming or relocating a test must regenerate `test-inventory.txt`; CI never
  rewrites either generated artifact.

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
