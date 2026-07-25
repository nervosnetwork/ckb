# Tx-Pool Test-Driven Review Guide

This guide is the reviewer entry point for tx-pool changes. It translates the
architectural invariants in [`security-regression-ledger.md`](security-regression-ledger.md)
into stable `TP-*` behaviors, hostile counterexamples and executable evidence.
The behavior/evidence mapping is generated from
[`review-behaviors.json`](review-behaviors.json); do not edit the generated
region by hand.

> Architecture status: the target model is normative in
> [`ARCHITECTURE.md`](ARCHITECTURE.md), its
> independent design audit is recorded in
> [`ARCHITECTURE_AUDIT.md`](ARCHITECTURE_AUDIT.md), and the staged migration is
> controlled by [`IMPLEMENTATION_PLAN.md`](IMPLEMENTATION_PLAN.md). Rows in this
> guide describe implemented behavior unless explicitly labelled as a target.

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

The stable proof obligation is: every transaction occupies exactly one owning
location at any instant, and every resident or borrowed resource is
continuously charged. At P1, the old coordinator/runtime/conflict-cache
encoding has been replaced by the seven-state `PrePoolKernel`; `EffectOutbox`
and the accepted-pool journal remain explicit P2/P3 migration debt. `TxPool`
is the sole accepted-state authority. Each phase must update a behavior row and
its evidence before deleting old encoding; derived indexes never become owners.

## Cross-authority gate

Apply this gate whenever a change touches more than one of `PrePoolKernel`,
`TxPool`, `EffectOutbox`, reorg recovery, persistence or block assembler:

- Identify the linearization point and prove there is no visible ownership gap
  or overlap.
- Write the lock/resource order explicitly. The target universal order is
  `optional permit -> TxPool -> PrePoolKernel`; no authority guard spans
  work/I/O/await. The C1 recovery order `recovery_lock -> effect credit ->
  TxPool` remains a compatibility constraint only until P4 deletes that
  protocol; no new path may depend on it.
- Prove every target failure is `Apply`, typed `Reject`, `Backpressure`, `Stale`
  or one bounded `Repair`. During migration, an old exact-undo/fail-stop path
  may only be deleted, never copied into the target. Hostile input may not
  reach service-wide fail-stop.
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

Process-level specs are required when their behavior row changes and must be
run through the generated `make integration CKB_TEST_ARGS='...'` command, not
by invoking a possibly stale `ckb-test` or `ckb` binary directly. The
`[integration]` inventory, behavior mapping and executable runner list must
agree. Benchmark timing is intentionally a separate final gate: deterministic
operation-count and harness-integrity tests run normally, but checkpoint A/B
timing must use the paired, fingerprinted runner and is performed only when
explicitly authorized. Unit-test duration is never accepted as performance
evidence.

## Registered behaviors and evidence

<!-- BEGIN GENERATED: TX_POOL_BEHAVIORS -->

### Managed process suite

The ten focused security anchors are the minimum process gate for the mapped behavior rows:

`make integration CKB_TEST_ARGS='-c 1 ReorgRecoversDependentPendingTree ReorgRecoversDependentChain ReorgRecoversDependentTxs RbfRejectReplaceProposed RbfOrphanRecovery RbfBasic RbfReplaceProposedSuccess RbfConcurrency RbfCyclingAttack RbfCellDepsCheck'`

The complete tx-pool impact universe contains 149 specs. P6 and release CI run the exact inventory through:

`make integration CKB_TEST_ARGS='-c 1 AvoidDuplicatedProposalsWithUncles BlockSyncDuplicatedAndReconnect BlockSyncForks BlockSyncFromOne BlockSyncNonAncestorBestBlocks BlockSyncOrphanBlocks BlockSyncRelayerCollaboration BlockSyncWithUncle BlockTemplates BlockTransactionsRelayParentOfOrphanBlock CellBeingCellDepAndSpentInSameBlockTestGetBlockTemplate CellBeingCellDepAndSpentInSameBlockTestGetBlockTemplateMultiple CellBeingCellDepThenSpentInSameBlockTestSubmitBlock CellBeingSpentThenCellDepInSameBlockTestSubmitBlock ChainFork1 ChainFork2 ChainFork3 ChainFork4 ChainFork5 ChainFork6 ChainFork7 CheckAbsoluteEpochSince CheckCellDeps CheckRelativeEpochSince CheckTypical2In2OutTx CheckVmBExtension CheckVmVersion1 CheckVmVersion2 CompactBlockEmpty CompactBlockEmptyParentUnknown CompactBlockLoseGetBlockTransactions CompactBlockMissingFreshTxs CompactBlockMissingNotFreshTxs CompactBlockMissingWithDropTx CompactBlockPrefilled CompactBlockRelayLessThenSharedBestKnown CompactBlockRelayParentOfOrphanBlock ConflictInGap ConflictInPending ConflictInProposed DAOWithSatoshiCellOccupied DeclaredWrongCycles DeclaredWrongCyclesAndRelayAgain DeclaredWrongCyclesChunk DepentTxInSameBlock DifferentTxsWithSameInputWithOutRBF DuplicatedTransaction FeeOfMaxBlockProposalsLimit FeeOfMultipleMaxBlockProposalsLimit FeeOfTransaction ForkedTransaction ForksContainSameTransactions ForksContainSameUncle GetRawTxPool HandlingDescendantsOfCommitted HandlingDescendantsOfProposed HeaderSyncCycle InboundMinedDuringSync InboundSync InvalidHeaderDep LoadProgramFailedTx LongForks MalformedTx MiningBasic NotifyLargeCyclesTx OrphanTxAccepted OrphanTxRejected OutboundMinedDuringSync OutboundSync PackUnclesIntoEpochStarting PoolPersisted PoolReconcile PoolResolveConflictAfterReorg PoolResurrect ProposalExpireRuleForCommittingAndExpiredAtOneTime ProposalRespondSizelimit ProposeButNotCommit ProposeDuplicated ProposeOutOfOrder ProposeTransactionButParentNot RbfBasic RbfCellDepsCheck RbfChildPayForParent RbfConcurrency RbfContainInvalidCells RbfContainInvalidInput RbfContainNewTx RbfCyclingAttack RbfEnable RbfOnlyForResolveDead RbfOrphanRecovery RbfRejectReplaceProposed RbfReplaceProposedSuccess RbfSameInput RbfSameInputwithLessFee RbfTooManyDescendants RelayInvalidTransaction RelayInvalidTransactionResumable RelayWithWrongTx RemoveConflictFromPending RemoveTx ReorgHandleProposals ReorgRecoversDependentChain ReorgRecoversDependentPendingTree ReorgRecoversDependentTxs RequestUnverifiedBlocks RpcGetBlockTemplate RpcSubmitBlock RpcTruncate SameCellAsInputAndCellDep SendConflictTxToRelay SendConflictTxToRelayRBF SendLargeCyclesTxInBlock SendLargeCyclesTxToRelay SendLowFeeRateTx SendTxChain SendTxChainRevOrder SizeLimit SpendSatoshiCell SubmitConflict SubmitTransactionWhenItsParentInGap SubmitTransactionWhenItsParentInProposed SyncTooNewBlock TooManyUnknownTransactions TransactionHashCollisionDifferentWitnessHashes TransactionRelayBasic TransactionRelayConflict TransactionRelayEmptyPeers TransactionRelayLowFeeRate TransactionRelayTimeout TxPoolEntryStatus TxPoolLimitAncestorCount TxPoolOrphanDoubleSpend TxPoolOrphanNormal TxPoolOrphanPartialInputUnknown TxPoolOrphanReverse TxPoolOrphanUnordered TxsRelayOrder UncleInheritFromForkBlock UncleInheritFromForkUncle ValidSince WithdrawDAO WithdrawDAOWithOverflowCapacity send_defected_binary_do_not_reject_known_bugs send_defected_binary_reject_known_bugs send_multisig_secp_tx_use_dep_group_data_hash send_multisig_secp_tx_use_dep_group_type_hash send_secp_tx_use_dep_group_data_hash send_secp_tx_use_dep_group_type_hash'`

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
| `TP-OWN-001` Single pre-pool ownership | `tx-pool/src/component/pre_pool`<br>`tx-pool/src/service/pipeline_ops.rs`<br>`tx-pool/src/service/workers.rs` | An admitted transaction has one full-hash PrePoolKernel entry, one of the frozen seven locations and one globally non-reused version until an atomic handoff transfers sole authority to TxPool. | A stale worker, duplicate admission, failed transition or ABA remove/readmit race must not create two owners, resurrect an old payload or silently erase the current owner. | I1, I4, I5, I6, I9, I12 | - Does every transition consume exactly the state and lease it proves current?<br>- Are every queue, deadline, dependency and conflict structure derived indexes rather than payload owners?<br>- Does failure restore the old owner or publish one explicit terminal outcome? | No second owner map, compensating queue, global post-transition scan or extra hot-path lock. |
| `TP-COMMIT-001` Authoritative commit and handoff | `tx-pool/src/process/submit`<br>`tx-pool/src/component/pre_pool/commit.rs`<br>`tx-pool/src/pool.rs` | The existing TxPool write guard is the only final membership/RBF sequencer; the current pool journal, kernel handoff, exact rollback and stable effects form one causally ordered boundary until P2 replaces them with total Plan/Apply. | Concurrent commits, handoff failure or panic must not expose a pool/kernel ownership gap, strand Ready work or report success for a rolled-back mutation. | I2, I4, I9 | - Is every final fee/conflict decision recomputed under the pool write guard?<br>- Can any error release the guard before coordinator settlement and pool rollback are exact?<br>- Is an uncertain authoritative mutation escalated instead of downgraded to a transaction reject? | Reuse the existing pool sequencer; do not add a normal-path recovery lock, second commit queue or population-sized reconciliation. |
| `TP-RBF-001` Deterministic RBF preference and rollback | `tx-pool/src/process/submit/rbf_commit.rs`<br>`tx-pool/src/component/pre_pool/commit.rs`<br>`tx-pool/src/component/pre_pool/wait.rs` | Only verified candidates participate in deterministic conflict ordering, while TxPool recomputes the complete replacement closure and both fee gates before atomic victim displacement. | An under-fee, multi-input, dep-group or concurrent candidate must not preempt through speculative state; failed replacement must restore the complete original closure before competitors advance. | I2, I3, I4, I9, I10, I11, I12 | - Is coordinator ordering still provisional rather than an admission verdict?<br>- Are all input and expanded dependency conflicts included in final closure and rollback?<br>- Does every failed path preserve original statuses, accounting and descendant order? | Conflict work stays within indexed bounded cohorts and immutable mutation plans; no full-pool scan under the write guard. |
| `TP-DEP-001` Causal dependency graph | `tx-pool/src/component/pre_pool/wait.rs`<br>`tx-pool/src/component/pre_pool/lifecycle.rs`<br>`tx-pool/src/resolved_tx.rs`<br>`tx-pool/src/component/links.rs` | Raw, resolved and accepted exact dependency keys—including headers and expanded dep-group members—remain one canonical primary fact: availability wakes children and definitive loss invalidates them atomically while by-parent is only a derived projection. | Late-discovered parents, transitive cycles, stale resolved children or parent replacement must not strand a child, lose its wake edge or let it commit against unavailable inputs. | I4, I6, I7, I9, I12 | - Are input, cell-dep, header-dep and expanded dep-group roles intentionally distinguished?<br>- Does parent success/failure update reverse edges, accounting and child location in one transition?<br>- Are cascade size and maintenance work explicitly bounded? | Use bounded indexed parent/child buckets and maintenance slices; never poll all waiting transactions or scan the pool for dependents. |
| `TP-CACHE-001` Conflict-history Wait ownership and wakeup | `tx-pool/src/component/pre_pool/wait.rs`<br>`tx-pool/src/component/pre_pool/runtime.rs`<br>`tx-pool/src/service/pipeline_ops.rs`<br>`tx-pool/src/process/reorg.rs` | Historical conflicts are ordinary bounded PrePoolKernel Wait entries until an exact dependency becomes available in the post-mutation TxPool/Snapshot overlay; waking changes the same primary owner atomically and is version-safe, while RPC projects retained conflict history through recent-reject rather than as live Pending work. | A release observed before another parent becomes live, an output created and consumed in one attached branch, duplicate metadata enrichment, remote conflict pinning or high-fanout input must not lose the only future wake, create a false wake, overwrite the root rejection, masquerade as Pending, retain epoch history without a waiter, duplicate ownership or cause unbounded pool-lock work. | I1, I2, I4, I5, I6, I9, I12 | - Does Wait retain ownership until an exact dependency epoch changes?<br>- Does every chain/pool availability edge re-arm a previously examined entry only when the resulting authoritative overlay makes that exact dependency available?<br>- Are discovery, generations and fanout work bounded and fair? | Bound history count/bytes and process indexed recovery in fixed fair slices outside population-sized scans. |
| `TP-BUDGET-001` Continuous hostile-state accounting | `tx-pool/src/component/pre_pool/mod.rs`<br>`tx-pool/src/component/pre_pool/runtime.rs`<br>`tx-pool/src/component/effect_outbox.rs`<br>`tx-pool/src/service/effects.rs` | Global and per-peer count, bytes and active-work budgets continuously charge payload and conservative metadata in every resident state, including bounded terminal effects. | Parking, invalidation, reservation, peer churn or an oversized displacement plan must not refund resident state, including the remote/per-peer charge of conflict history, evict unrelated stronger work or mutate before proving the bound. | I4, I5, I12 | - Is every owner charged if and only if it is resident?<br>- Are count, bytes, graph edges, victims and active work all bounded before mutation?<br>- Does an impossible peer admission fail before global eviction planning? | Budget checks and victim selection use maintained bounded indexes; no attacker-sized repair on the admission hot path. |
| `TP-WORKER-001` Level-triggered executable readiness | `tx-pool/src/component/pre_pool/queue.rs`<br>`tx-pool/src/component/pre_pool/runtime.rs`<br>`tx-pool/src/service/workers.rs`<br>`tx-pool/src/service/builder.rs` | Readiness is derived after each transition from the authoritative capability-aware checkout predicate; failed ineligible checkout is silent and subscription/respawn re-arms executable work. | A capped peer backlog must not self-wake into mutex-starving livelock; a small-only worker must not consume the only wake for large work; cancellation, zero workers, a consumed Notify before panic or respawn must not strand executable work. | I4, I12 | - Does notification mean at least one worker of this capability can execute now?<br>- Can a consumed permit be reconstructed from authoritative state after subscribe or respawn?<br>- Does cancellation stop checkout before another self-sustaining wake loop begins? | Readiness checks inspect bounded owner heads/caps only; no polling loop, per-item task or queue-wide scan. |
| `TP-ADMIN-001` Administrative and hostile-peer terminalization | `tx-pool/src/service.rs`<br>`tx-pool/src/service/pipeline_ops.rs`<br>`tx-pool/src/component/recent_reject.rs` | Ban, expiry, clear and malformed-input policy terminalize the current non-committing owner once, release all resource/index state and publish only policy-eligible effects. | An already active malicious peer must not pin an active slot, resurrect through a late lease or turn a typed malformed input into fail-stop or an ineligible relay reject. | I4, I5, I8 | - Does administrative removal make every outstanding lease stale atomically?<br>- Are trusted promotion and immutable ingress attribution preserved deliberately?<br>- Are ban/reject history and relay policy separate explicit decisions? | Administrative removal uses indexed owners and bounded batches; peer fences/history remain bounded and expiring. |
| `TP-EFFECT-001` Reserved stable-state effects | `tx-pool/src/component/effect_outbox.rs`<br>`tx-pool/src/service/effects.rs`<br>`tx-pool/src/callback.rs` | Mutation-coupled effects reserve capacity before state change, enter one FIFO sequence while state is stable and publish outside authority locks; capacity/close waits cannot lose wakeups. | A full or panicking consumer, callback re-entry, close race or check-before-wait window must not expose intermediate state, reorder outcomes, lose charges or sleep forever. | I5, I8, I12 | - Was sufficient effect credit reserved before every coupled mutation?<br>- Is publication outside locks while FIFO ownership and charge remain retained?<br>- Do waiters register before checking and does close use the correct stored/broadcast wake semantic? | One bounded publisher/outbox; no unbounded channel, callback-under-lock, busy retry or task per effect. |
| `TP-REORG-001` Serialized reliable chain transitions | `tx-pool/src/process/reorg.rs`<br>`tx-pool/src/service.rs`<br>`tx-pool/src/service/pipeline_ops.rs` | Reorg deltas remain ordered and retained through retry; recovery_lock serializes complete detached replay, clear and persistence, while final effects describe only authoritative post-replay state. | Repeated callback panic, duplicate detached roots, clear/save races or partial replay must not drop/overtake a delta, expose an ownership gap, deadlock effect credit or persist an intermediate pool. | I1, I2, I4, I6, I7, I8, I11 | - Is lock order recovery_lock then effect credit then TxPool preserved?<br>- Can retry replay an already completed authoritative phase or an obsolete snapshot?<br>- Are duplicate recovery, final-state effects and attached/detached identities exact? | Retain a capacity-one ordered delta and bounded replay slices; no independent recovery worker or full-history duplication. |
| `TP-PERSIST-001` Coherent persistence recovery point | `tx-pool/src/service.rs`<br>`tx-pool/src/component/pool_map.rs`<br>`tx-pool/src/process/reorg.rs` | Persistence snapshots only a coherent accepted pool after complete recovery, orders by the authoritative expanded dependency graph and is disabled only when authoritative mutation may be uncertain. | Save racing detached replay, effect-journal failure or expanded dep-group ordering must not persist half a reorg, lose a recoverable pool or serialize children before required parents. | I7, I8, I10 | - Does save hold recovery serialization across its complete snapshot boundary?<br>- Is PoolMap the only ordering authority, including expanded dependencies?<br>- Does each failure domain make the intended persistence decision? | Shutdown-only clone/sort work stays off admission paths; do not add a continuously maintained persistence projection. |
| `TP-POOL-001` Atomic accepted-pool graph integrity | `tx-pool/src/component/pool_map.rs`<br>`tx-pool/src/component/links.rs`<br>`tx-pool/src/pool.rs` | Pool entries, status counters, dependency links, conflict closure, ancestor/descendant weights and exact rollback journals mutate as one authoritative graph. | Ghost links, counter drift, late-parent insertion, expired-parent cascades or escape-hatch eviction must not corrupt graph weights, preserve impossible children or remove required ancestors. | I2, I9, I10 | - Does one immutable plan cover the complete mutation before any write?<br>- Does the independent audit rebuild agree after success and rollback?<br>- Are required parents distinguished from ordering-only references? | Mutation and audit work is bounded by explicit graph/victim caps; cold repair must never hide hot-path drift. |
| `TP-TEMPLATE-001` Block-template liveness and priority | `tx-pool/src/block_assembler`<br>`tx-pool/src/pool.rs`<br>`tx-pool/src/process/reorg.rs` | Reset and full rebuild remain mutually exclusive with full highest priority; optimistic proposal/transaction updates are revision-safe, and valid recovered transactions always regain a proposal/commit path. | Detached uncle proposals, stale Gap status or an update_full race must not make a valid transaction RPC-pending but forever absent from normal get_block_template mining. | I2, I7, I9, I11 | - Does full/reset retain authority over every optimistic delta and re-dirty skipped generations?<br>- Can uncle proposal filtering exclude the sole proposal path of a recovered transaction?<br>- Are Gap/Pending/Proposed transitions reflected to assembler selection? | Keep optimistic CAS updates and bounded selection; do not serialize every delta behind full rebuild or remove bounded packing safeguards without measurements. |
| `TP-IDENTITY-001` Full transaction and witness identity | `tx-pool/src/process/submit/mod.rs`<br>`tx-pool/src/component/pre_pool/mod.rs`<br>`tx-pool/src/component/pool_map.rs` | Ownership and duplicate boundaries use full raw hashes, proposal short IDs remain non-authoritative indexes, and verification-cache proofs are keyed only by the exact witness hash through TxVerificationCacheKey. | Short-ID collisions or same-raw/different-witness variants must not alias accepted/cache/history ownership, obtain a false duplicate success or reuse an invalid verification proof during reorg. | I1, I2, I4, I5, I7, I9, I10 | - Is a short ID used only where collision-aware lookup semantics are explicit?<br>- Can any cache call construct a key from raw hash or arbitrary bytes?<br>- Does reorg recovery query the exact transaction witness variant? | Use compact full hashes/typed cache keys without retaining packed backing; collision handling remains indexed and bounded. |
| `TP-PERF-001` Bounded attacker-controlled work | `tx-pool/src/component/pre_pool/queue.rs`<br>`tx-pool/src/component/pre_pool/commit.rs`<br>`tx-pool/src/benchmark.rs`<br>`devtools/tx_pool_bench.py` | Owner-head scheduling, victim selection and conflict probing stop at maintained bounds independent of unrelated population; benchmark comparisons use fingerprinted paired checkpoints and reject noisy samples. | A capped peer prefix, stronger suffix or large independent population must not turn one admission/checkout into an O(pool) scan; a noisy or mismatched harness must not claim a performance win. | I5, I12 | - Is operation count bounded by owners/cohort/config rather than resident transaction count?<br>- Did the change add allocation, lock, task, scan or mutable projection to a hot path?<br>- When benchmarking is authorized, are worktree, binary, config, repetitions and spread comparable? | Deterministic operation-count regressions are always required; timing A/B is the separately authorized final gate, never inferred from unit duration. |

### Executable evidence

#### `TP-OWN-001` — Single pre-pool ownership

Minimum command: `cargo nextest run -p ckb-tx-pool --lib -E 'test(/(concrete_kernel_transitions|stale_lease_cannot_mutate|randomized_public_transitions|target_model_generated_commands)/)'`

Rust evidence:

- `concrete_kernel_transitions_preserve_recomputed_projections` (I1, I4, I5, I6, I9)
- `randomized_public_transitions_always_match_full_rebuild` (I1, I4, I5, I6, I9, I12)
- `stale_lease_cannot_mutate_a_removed_and_readmitted_hash` (I1, I4)
- `target_model_declares_exactly_the_frozen_seven_states` (I1)
- `target_model_generated_commands_preserve_partition_lease_budget_and_indexes` (I1, I4, I5, I6, I9)
- `target_model_stale_lease_cannot_mutate_a_replaced_witness_owner` (I1, I4)

#### `TP-COMMIT-001` — Authoritative commit and handoff

Minimum command: `cargo nextest run -p ckb-tx-pool --lib -E 'test(/(pipeline_commit_worker|local_commit_waits|target_model_exercises_every_plan_outcome|failed_commit_restores)/)'`

Rust evidence:

- `pipeline_commit_worker_waits_for_the_pool_sequencer` (I2, I9)
- `target_model_exercises_every_plan_outcome_without_partial_mutation` (I2, I4)

#### `TP-RBF-001` — Deterministic RBF preference and rollback

Minimum command: `cargo nextest run -p ckb-tx-pool --lib -E 'test(/(rbf|replacement|pipeline_rejects_conflicting_double_spend|full_conflict_history)/)'`

Rust evidence:

- `escape_hatch_evictions_are_recovered_on_commit_failure` (I3, I4, I10)
- `failed_commit_restores_all_size_evictions_with_original_status_in_lock` (I3, I10)
- `pipeline_rejects_conflicting_double_spend` (I2, I9)
- `rbf_rejects_dep_group_member_from_replacement_victim` (I2, I9, I10)
- `rbf_replacement_certain_to_fail_commit_cannot_churn_pool` (I2, I4, I9, I12)
- `successful_replacement_does_not_recover_removed_descendants` (I2, I9)

Process-level evidence:

- `rbf-basic`: `test/src/specs/tx_pool/replace.rs::RbfBasic` (I2, I4, I9) — The node enforces the replacement fee rule, exposes the victim as RBFRejected, commits the accepted higher-fee replacement, and does not reinterpret an output created and consumed in that attached branch as an availability edge that overwrites the root rejection. Paired units: `successful_replacement_does_not_recover_removed_descendants`, `dependency_availability_uses_the_authoritative_overlay_level`. Command: `make integration CKB_TEST_ARGS='-c 1 RbfBasic'`
- `rbf-cell-deps`: `test/src/specs/tx_pool/replace.rs::RbfCellDepsCheck` (I2, I9, I10) — Final node admission rejects a replacement that depends on a cell from its victim closure and records the failed candidate without corrupting the accepted pool graph. Paired units: `rbf_rejects_dep_group_member_from_replacement_victim`. Command: `make integration CKB_TEST_ARGS='-c 1 RbfCellDepsCheck'`
- `rbf-concurrency`: `test/src/specs/tx_pool/replace.rs::RbfConcurrency` (I2, I9) — Concurrent RPC submissions for one input converge to the unique highest-fee transaction and record every losing candidate as rejected without a recovery livelock. Paired units: `pipeline_rejects_conflicting_double_spend`, `successful_replacement_does_not_recover_removed_descendants`. Command: `make integration CKB_TEST_ARGS='-c 1 RbfConcurrency'`
- `rbf-conflict-recovery`: `test/src/specs/tx_pool/orphan_tx_recovery.rs::RbfOrphanRecovery` (I4, I9) — When a replacement frees only one of two historical inputs, the node recovers exactly the newly eligible victim and retains the still-conflicting victim as rejected. Paired units: `full_conflict_history_terminalizes_rejected_owner_without_panicking`, `successful_replacement_does_not_recover_removed_descendants`. Command: `make integration CKB_TEST_ARGS='-c 1 RbfOrphanRecovery'`
- `rbf-cycling-attack`: `test/src/specs/tx_pool/replace.rs::RbfCyclingAttack` (I4, I9) — A cyclic sequence of overlapping replacements recovers the transaction whose input becomes free and does not resurrect transactions whose inputs remain consumed. Paired units: `full_conflict_history_terminalizes_rejected_owner_without_panicking`, `successful_replacement_does_not_recover_removed_descendants`. Command: `make integration CKB_TEST_ARGS='-c 1 RbfCyclingAttack'`
- `rbf-proposed-success`: `test/src/specs/tx_pool/replace.rs::RbfReplaceProposedSuccess` (I2, I9, I11) — A higher-fee replacement of a Proposed chain member survives a subsequent blank block, is freshly proposed, and is committed while the displaced closure remains rejected. Paired units: `successful_replacement_does_not_recover_removed_descendants`, `commit_and_removal_journal_block_assembler_delta`. Command: `make integration CKB_TEST_ARGS='-c 1 RbfReplaceProposedSuccess'`
- `rbf-proposed-template-refresh`: `test/src/specs/tx_pool/replace.rs::RbfRejectReplaceProposed` (I2, I9, I11) — Replacing a Proposed transaction rejects the victim, refreshes the real node template, and normally proposes and commits only the replacement. Paired units: `successful_replacement_does_not_recover_removed_descendants`, `commit_and_removal_journal_block_assembler_delta`. Command: `make integration CKB_TEST_ARGS='-c 1 RbfRejectReplaceProposed'`

#### `TP-DEP-001` — Causal dependency graph

Minimum command: `cargo nextest run -p ckb-tx-pool --lib -E 'test(/(parent_commit_before_wait|parent_loss|successive_expanded_parent_losses|dependency_epochs|dependency|dep_group|unknown_outpoint)/)'`

Rust evidence:

- `local_rbf_commit_demotes_consumer_of_live_expanded_dep_group_member` (I6)
- `parent_commit_before_wait_registration_requeues_child` (I4, I6)
- `parent_loss_invalidates_an_active_lease_into_exact_wait` (I4, I6)
- `remote_parent_wait_and_unknown_parents_effect_are_one_transition` (I4)
- `repeated_dependency_epochs_are_level_triggered_and_bounded` (I4, I6, I12)
- `resolve_job_registers_the_exact_unknown_outpoint` (I4, I6)
- `sort_txs_by_dependencies_orders_parents_before_children` (I6)
- `successive_expanded_parent_losses_keep_exact_causal_keys_in_wait` (I4, I6)
- `target_model_wait_wake_and_ready_conflict_use_recomputed_views` (I6, I9)

Process-level evidence:

- `dependent-reorg-chain`: `test/src/specs/tx_pool/reorg_recovers_dependent.rs::ReorgRecoversDependentChain` (I6, I7) — A three-level detached dependency chain is replayed parent-first and every member returns to a committable pool state after the node adopts the competing fork. Paired units: `sort_txs_by_dependencies_orders_parents_before_children`, `reorg_direct_replay_treats_pool_duplicates_as_idempotent`. Command: `make integration CKB_TEST_ARGS='-c 1 ReorgRecoversDependentChain'`
- `dependent-reorg-transactions`: `test/src/specs/tx_pool/reorg_recovers_dependent.rs::ReorgRecoversDependentTxs` (I6, I7) — A detached parent and its dependent child are both recovered through the node reorg callback and remain committable as one dependency-ordered pool graph. Paired units: `sort_txs_by_dependencies_orders_parents_before_children`, `reorg_direct_replay_treats_pool_duplicates_as_idempotent`. Command: `make integration CKB_TEST_ARGS='-c 1 ReorgRecoversDependentTxs'`

#### `TP-CACHE-001` — Conflict-history Wait ownership and wakeup

Minimum command: `cargo nextest run -p ckb-tx-pool --lib -E 'test(/(full_conflict_history|repeated_dependency_epochs|availability_without_a_wait|dependency_availability_uses_the_authoritative_overlay|remote_conflict_history|remote_peer_order|pipeline_rejects_conflicting_double_spend)/)'`

Rust evidence:

- `availability_without_a_wait_owner_retains_no_epoch_history` (I5, I12)
- `dependency_availability_uses_the_authoritative_overlay_level` (I4, I6, I9)
- `full_conflict_history_terminalizes_rejected_owner_without_panicking` (I4, I5, I9)
- `remote_conflict_history_keeps_its_bounded_residency_deadline` (I4, I5)
- `remote_peer_order_cannot_hijack_an_existing_conflict_owner` (I1, I9)

Process-level evidence:

- `rbf-basic`: `test/src/specs/tx_pool/replace.rs::RbfBasic` (I2, I4, I9) — The node enforces the replacement fee rule, exposes the victim as RBFRejected, commits the accepted higher-fee replacement, and does not reinterpret an output created and consumed in that attached branch as an availability edge that overwrites the root rejection. Paired units: `successful_replacement_does_not_recover_removed_descendants`, `dependency_availability_uses_the_authoritative_overlay_level`. Command: `make integration CKB_TEST_ARGS='-c 1 RbfBasic'`
- `rbf-conflict-recovery`: `test/src/specs/tx_pool/orphan_tx_recovery.rs::RbfOrphanRecovery` (I4, I9) — When a replacement frees only one of two historical inputs, the node recovers exactly the newly eligible victim and retains the still-conflicting victim as rejected. Paired units: `full_conflict_history_terminalizes_rejected_owner_without_panicking`, `successful_replacement_does_not_recover_removed_descendants`. Command: `make integration CKB_TEST_ARGS='-c 1 RbfOrphanRecovery'`
- `rbf-cycling-attack`: `test/src/specs/tx_pool/replace.rs::RbfCyclingAttack` (I4, I9) — A cyclic sequence of overlapping replacements recovers the transaction whose input becomes free and does not resurrect transactions whose inputs remain consumed. Paired units: `full_conflict_history_terminalizes_rejected_owner_without_panicking`, `successful_replacement_does_not_recover_removed_descendants`. Command: `make integration CKB_TEST_ARGS='-c 1 RbfCyclingAttack'`

#### `TP-BUDGET-001` — Continuous hostile-state accounting

Minimum command: `cargo nextest run -p ckb-tx-pool --lib -E 'test(/(admission_budget_failure|parent_loss_uses_the_continuous_wait_reservation|remote_conflict_keeps_remote_reservation|owner_fairness_and_active_caps|count_and_byte_limits|escape_hatch_rejects)/)'`

Rust evidence:

- `admission_budget_failure_leaves_primary_and_views_unchanged` (I4, I5)
- `count_and_byte_limits_cover_reserved_queued_and_active_batches` (I5)
- `escape_hatch_rejects_mutation_larger_than_displacement_bound` (I5, I12)
- `parent_loss_uses_the_continuous_wait_reservation_at_a_full_budget` (I4, I5)
- `remote_conflict_keeps_remote_reservation_and_wakes_without_capacity_retry` (I4, I5, I12)
- `verified_candidate_compacts_deps_and_pool_budget_counts_retained_inputs` (I5)

#### `TP-WORKER-001` — Level-triggered executable readiness

Minimum command: `cargo nextest run -p ckb-tx-pool --lib -E 'test(/(owner_fairness_and_active_caps|ready_expiry_filter|worker_|zero_verify_worker)/)'`

Rust evidence:

- `cancel_during_backoff_exits_immediately` (I4, I12)
- `ready_expiry_filter_does_not_starve_later_non_ready_deadlines` (I4, I12)
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
- `reorg_direct_replay_treats_pool_duplicates_as_idempotent` (I4, I7)
- `reorg_status_transition_failure_has_no_false_reject_and_replay_converges` (I7)
- `retained_receiver_preserves_fifo_across_panics` (I7)

Process-level evidence:

- `dependent-reorg-chain`: `test/src/specs/tx_pool/reorg_recovers_dependent.rs::ReorgRecoversDependentChain` (I6, I7) — A three-level detached dependency chain is replayed parent-first and every member returns to a committable pool state after the node adopts the competing fork. Paired units: `sort_txs_by_dependencies_orders_parents_before_children`, `reorg_direct_replay_treats_pool_duplicates_as_idempotent`. Command: `make integration CKB_TEST_ARGS='-c 1 ReorgRecoversDependentChain'`
- `dependent-reorg-transactions`: `test/src/specs/tx_pool/reorg_recovers_dependent.rs::ReorgRecoversDependentTxs` (I6, I7) — A detached parent and its dependent child are both recovered through the node reorg callback and remain committable as one dependency-ordered pool graph. Paired units: `sort_txs_by_dependencies_orders_parents_before_children`, `reorg_direct_replay_treats_pool_duplicates_as_idempotent`. Command: `make integration CKB_TEST_ARGS='-c 1 ReorgRecoversDependentTxs'`
- `normal-mining-reorg-tree`: `test/src/specs/tx_pool/reorg_recovers_dependent.rs::ReorgRecoversDependentPendingTree` (I7, I11) — A detached dependent tree is recovered as real Pending and is re-proposed and committed by normal get_block_template mining without relying on optional uncles or a hand-authored proposal block. Paired units: `reorg_status_transition_failure_has_no_false_reject_and_replay_converges`, `reorg_demotes_stale_gap_to_pending`, `pending_proposals_filter_conflicting_uncle_subtree`. Command: `make integration CKB_TEST_ARGS='-c 1 ReorgRecoversDependentPendingTree'`

#### `TP-PERSIST-001` — Coherent persistence recovery point

Minimum command: `cargo nextest run -p ckb-tx-pool --features internal -E 'test(/(save_pool_waits|stable_effect_journal|persisted_file_orders)/)'`

Rust evidence:

- `dispatcher_channel_close_quiesces_workers_and_persists_pool` (I7, I8)
- `persisted_file_orders_expanded_dep_group_parents` (I7, I10)
- `save_pool_waits_for_complete_reorg_recovery_point` (I7)

#### `TP-POOL-001` — Atomic accepted-pool graph integrity

Minimum command: `cargo nextest run -p ckb-tx-pool --features internal -E 'test(/(conflict_closure_ignores|status_counter_underflow|escape_hatch_never|test_dep_group|parent_added_after_child|reorg_expiry_cascades)/)'`

Rust evidence:

- `conflict_closure_ignores_ghost_link_nodes` (I10)
- `escape_hatch_never_evicts_a_required_parent` (I10)
- `parent_added_after_child_gets_descendant_weight` (I10)
- `reorg_expiry_cascades_from_expired_parent_to_fresh_child` (I10)
- `status_counter_underflow_recomputes_from_authoritative_entries` (I10)
- `test_dep_group` (I10)

Process-level evidence:

- `rbf-cell-deps`: `test/src/specs/tx_pool/replace.rs::RbfCellDepsCheck` (I2, I9, I10) — Final node admission rejects a replacement that depends on a cell from its victim closure and records the failed candidate without corrupting the accepted pool graph. Paired units: `rbf_rejects_dep_group_member_from_replacement_victim`. Command: `make integration CKB_TEST_ARGS='-c 1 RbfCellDepsCheck'`

#### `TP-TEMPLATE-001` — Block-template liveness and priority

Minimum command: `cargo nextest run -p ckb-tx-pool --features internal -E 'test(/(reorg_demotes_stale_gap|pending_proposals_filter|full_and_uncle_updates)/)'`

Rust evidence:

- `commit_and_removal_journal_block_assembler_delta` (I11)
- `full_and_uncle_updates_share_template_serialization_lock` (I11)
- `pending_proposals_filter_conflicting_uncle_subtree` (I11)
- `reorg_demotes_stale_gap_to_pending` (I11)

Process-level evidence:

- `normal-mining-reorg-tree`: `test/src/specs/tx_pool/reorg_recovers_dependent.rs::ReorgRecoversDependentPendingTree` (I7, I11) — A detached dependent tree is recovered as real Pending and is re-proposed and committed by normal get_block_template mining without relying on optional uncles or a hand-authored proposal block. Paired units: `reorg_status_transition_failure_has_no_false_reject_and_replay_converges`, `reorg_demotes_stale_gap_to_pending`, `pending_proposals_filter_conflicting_uncle_subtree`. Command: `make integration CKB_TEST_ARGS='-c 1 ReorgRecoversDependentPendingTree'`
- `rbf-proposed-success`: `test/src/specs/tx_pool/replace.rs::RbfReplaceProposedSuccess` (I2, I9, I11) — A higher-fee replacement of a Proposed chain member survives a subsequent blank block, is freshly proposed, and is committed while the displaced closure remains rejected. Paired units: `successful_replacement_does_not_recover_removed_descendants`, `commit_and_removal_journal_block_assembler_delta`. Command: `make integration CKB_TEST_ARGS='-c 1 RbfReplaceProposedSuccess'`
- `rbf-proposed-template-refresh`: `test/src/specs/tx_pool/replace.rs::RbfRejectReplaceProposed` (I2, I9, I11) — Replacing a Proposed transaction rejects the victim, refreshes the real node template, and normally proposes and commits only the replacement. Paired units: `successful_replacement_does_not_recover_removed_descendants`, `commit_and_removal_journal_block_assembler_delta`. Command: `make integration CKB_TEST_ARGS='-c 1 RbfRejectReplaceProposed'`

#### `TP-IDENTITY-001` — Full transaction and witness identity

Minimum command: `cargo nextest run -p ckb-tx-pool --lib -E 'test(/(short_id_collision|full_hash_lookup|proposal_witness_variant|trusted_conflict_resubmission|verification_cache_isolated|reorg_recovery_reads_cache)/)'`

Rust evidence:

- `full_hash_lookup_does_not_alias_a_proposal_short_id_collision` (I1, I10)
- `pool_short_id_collision_is_not_a_successful_duplicate` (I1, I2, I10)
- `proposal_witness_variant_replaces_remote_payload_at_authoritative_handoff` (I1, I4, I5)
- `reorg_recovery_reads_cache_by_exact_witness_hash` (I1, I7, I9)
- `short_id_collision_is_backpressure_not_aliasing` (I1, I5)
- `synchronous_precheck_does_not_alias_short_id_collision_as_duplicate` (I1, I4)
- `trusted_conflict_resubmission_refreshes_the_exact_witness_owner` (I1, I9)
- `verification_cache_isolated_by_witness_hash_not_raw_hash` (I1, I9)

#### `TP-PERF-001` — Bounded attacker-controlled work

Minimum command: `cargo nextest run -p ckb-tx-pool --lib -E 'test(/(owner_fairness_and_active_caps|ready_expiry_filter|randomized_public_transitions)/)'`

Rust evidence:

- `conflict_closure_aborts_at_candidate_limit` (I5, I12)
- `descendants_cache_members_stay_within_budget` (I12)
- `owner_fairness_and_active_caps_do_not_scan_a_capped_prefix` (I5, I12)

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
