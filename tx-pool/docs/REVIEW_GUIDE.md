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

When `TP-BUDGET-001` changes the RelayV3 transaction-batch boundary, also run:

```text
cargo nextest run -p ckb-sync -E 'test(block_proposal_process)'
```

These tests prove that count and byte validation happens before proposal
inflight/known state changes, while valid and expired proposal paths keep their
existing behavior.

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

## Registered behaviors and evidence

<!-- BEGIN GENERATED: TX_POOL_BEHAVIORS -->

### Managed process suite

The 17 focused security anchors are the minimum process gate for the mapped behavior rows:

`make integration CKB_TEST_ARGS='-c 1 CellBeingCellDepAndSpentInSameBlockTestGetBlockTemplate TxPoolLimitAncestorCount ReorgRecoversDependentPendingTree ReorgRecoversDependentChain ReorgRecoversDependentTxs PoolResolveConflictAfterReorg RbfRejectReplaceProposed RbfOrphanRecovery RbfBasic RbfReplaceProposedSuccess RbfConcurrency RbfCyclingAttack RbfCellDepsCheck TxPoolOrphanReverse TxsRelayOrder LocalTestSubmissionIsDirect UncleInheritFromForkUncle'`

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
| `TP-OWN-001` Single pre-pool ownership | `tx-pool/src/component/pre_pool`<br>`tx-pool/src/service/pipeline_ops.rs`<br>`tx-pool/src/service/workers.rs` | An admitted transaction has one full-hash PrePoolKernel entry, one of the frozen six locations and one globally non-reused revision for its exact primary state and derived indexes until an atomic handoff transfers sole authority to TxPool. Resolve and Verify settlement accept only their corresponding typed lease; callers cannot construct a raw revision/location pair. | A stale worker, duplicate admission, failed transition or ABA remove/readmit race must not create two owners, resurrect an old payload or silently erase the current owner. | T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13 | - Does every transition consume exactly the state and lease it proves current?<br>- Can any worker settlement accept a caller-assembled hash/revision/location tuple instead of a typed ResolveLease or VerifyLease?<br>- Are every queue, deadline, dependency and conflict structure derived indexes rather than payload owners?<br>- Does failure restore the old owner or publish one explicit terminal outcome? | No second owner map, compensating queue, global post-transition scan or extra hot-path lock. |
| `TP-COMMIT-001` Authoritative commit and handoff | `tx-pool/src/process/submit`<br>`tx-pool/src/component/pre_pool/commit.rs`<br>`tx-pool/src/pool.rs`<br>`tx-pool/src/authority/chain.rs`<br>`tx-pool/src/authority/validation.rs`<br>`tx-pool/src/authority/runtime.rs`<br>`tx-pool/src/authority/plan.rs`<br>`tx-pool/src/authority/plan/settlement.rs` | The TxPool write guard is the only final membership/RBF sequencer. It builds one immutable AdmissionPlan containing the accepted PoolMutationPlan, matching revision-bound kernel handoff, exact effect batch and template receipt; one total Apply moves the logical owner and journals those committed effects. A verified lease may fold its otherwise adjacent Ready handoff into that same sealed CommitSession only when its prospective ReadyKey is stronger than every published Ready owner and its exact transient payload charge fits the existing budgets; every Plan, ban fence, rejection and effect path remains shared. Tx-pool final admission may reuse positive resolved-cell evidence only per cell, after the mutable pool overlay, when the resolution and current tip hashes match and transaction_info proves chain provenance. | Concurrent commits, a stronger Ready owner, stale leases, rejected RBF, transient verified-payload growth, journal backpressure, accepted-duplicate acknowledgement racing clear/reorg, or a structural defect must not expose a pool/kernel ownership gap, strand Ready work, bypass ordering/budgets, or publish success for an absent/unapplied owner. | T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11 | - Is every final fee/conflict decision recomputed under the pool write guard?<br>- Does verified-to-accepted folding reuse the sealed pipeline CommitSession, exact payload-derived charge and the ordinary Ready fallback instead of adding a second admission path?<br>- Can a stronger published Ready owner, peer-ban fence or unavailable effect journal force the verified owner through the unchanged Ready/terminal transition before any Apply?<br>- Is same-tip resolved-cell evidence confined to tx-pool admission, chain-provenanced per cell, and excluded from removal-history and block/consensus validation?<br>- Can any ordinary error occur after accepted-pool Apply begins?<br>- Is an uncertain authoritative mutation escalated instead of downgraded to a transaction reject? | Reuse the existing pool sequencer and fold only the adjacent verified/Ready ceremony; do not add a normal-path recovery lock, second admission implementation, commit queue or population-sized reconciliation. |
| `TP-RBF-001` Deterministic mutation-free RBF planning | `tx-pool/src/process/submit/rbf_commit.rs`<br>`tx-pool/src/component/pre_pool/commit.rs`<br>`tx-pool/src/component/pre_pool/wait.rs` | Only verified candidates participate in deterministic conflict ordering, while TxPool recomputes the complete replacement closure and both fee gates before atomic victim displacement. | An under-fee, multi-input, dep-group, self-evicted or concurrent candidate must not preempt through speculative state; every failed replacement must leave the complete original pool unchanged. | T1, T2, T3, T4, T5, T6, T7, T8, T10, T11, T12, T13 | - Is pre-pool Ready ordering still provisional rather than an admission verdict?<br>- Are all input and expanded dependency conflicts included in the immutable removal union?<br>- Does every failed path preserve original statuses, accounting and descendant order? | Conflict work stays within indexed bounded cohorts and immutable mutation plans; no full-pool scan under the write guard. |
| `TP-DEP-001` Causal dependency graph | `tx-pool/src/component/pre_pool/wait.rs`<br>`tx-pool/src/component/pre_pool/lifecycle.rs`<br>`tx-pool/src/resolved_tx.rs`<br>`tx-pool/src/component/links.rs`<br>`tx-pool/src/pool_cell.rs`<br>`tx-pool/src/component/tx_selector.rs`<br>`tx-pool/src/authority/chain.rs`<br>`tx-pool/src/authority/dependency.rs`<br>`tx-pool/src/authority/work.rs`<br>`tx-pool/src/authority/plan.rs`<br>`tx-pool/src/authority/effect.rs`<br>`tx-pool/src/authority/runtime.rs` | Raw, resolved and accepted exact dependency keys—including the complete direct missing-cell frontier, headers and expanded dep-group members—remain one canonical primary fact: availability wakes children and definitive loss invalidates them atomically while by-parent is only a derived projection. A Remote transition to Waiting(Missing) commits one canonical transaction-parent request in the same Apply; trusted sources retain the same wait without manufacturing network work. | A cell provider reporting only its first miss, duplicate missing outputs from one parent, late-discovered parents, effect backpressure, conditional reader/spender cycles, reversed arrival order, stale resolved children or parent replacement must not strand a child, make RelayV3 reject an unrequested parent, hot-loop resolution, lose its wake edge or let a template commit an invalid order. | T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13 | - Are input, cell-dep, header-dep and expanded dep-group roles intentionally distinguished?<br>- Does parent success/failure update reverse edges, accounting and child location in one transition?<br>- Are cascade size and maintenance work explicitly bounded? | Use bounded indexed parent/child buckets and maintenance slices; never poll all waiting transactions or scan the pool for dependents. |
| `TP-CACHE-001` Bounded conflict-history ownership and wakeup | `tx-pool/src/component/pre_pool/wait.rs`<br>`tx-pool/src/component/pre_pool/lifecycle.rs`<br>`tx-pool/src/component/pre_pool/stored_entry.rs`<br>`tx-pool/src/component/pre_pool/runtime.rs`<br>`tx-pool/src/service/pipeline_ops.rs`<br>`tx-pool/src/process/reorg.rs`<br>`tx-pool/src/authority/state.rs`<br>`tx-pool/src/authority/plan.rs`<br>`tx-pool/src/authority/plan/membership.rs`<br>`tx-pool/src/authority/dependency.rs`<br>`tx-pool/src/authority/read.rs` | Historical conflicts have one bounded retained owner until every exact dependency that is unavailable in the replacement's projected final TxPool/Snapshot overlay has a newer final availability level. In the UAK, only a genuinely Accepted replacement victim can enter the dedicated ReplacementHistory owner type; it has no source, peer, deadline, scheduler lane or executable phase. The full dependency basis remains retained for fresh validation, but only final blockers are wake triggers. Unchanged history observes the next dependency level, while a victim retained by the same atomic cohort is sealed at that cohort's post-Apply observation cut and cannot self-wake. The complete replacement closure is retained or terminalized as one charged set, waking changes the same primary owner atomically and is version-safe, and RPC projects retained history through existing recent-reject/RBF semantics rather than as live Pending work. | A partial release while another input remains blocked, a release observed before another parent becomes live, a newly retained victim observing its own replacement release, an unrelated chain dependency changing while the winner still owns the true conflict, an output created and consumed in one attached branch, duplicate metadata enrichment, remote conflict pinning or high-fanout input must not lose the only future wake, create a false wake, overwrite the root rejection, masquerade as Pending, retain epoch history without a waiter, duplicate ownership or cause unbounded pool-lock work. | T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13 | - Can only an Accepted replacement victim construct the non-executable ReplacementHistory owner, while failed/under-fee candidates remain structurally unable to acquire it?<br>- Does the cohort seal distinguish an unchanged historical waiter from a conflict victim created by that same Apply, without hand-coded call-site ordering?<br>- Does every chain/pool availability edge re-arm a previously examined entry only after the resulting authoritative levels make every originally unavailable blocker available?<br>- Does resource pressure drop the complete optional history set without failing the winner or publishing a partial set?<br>- Are discovery, generations and fanout work bounded and fair, and are history owners excluded from Pending/template/persistence? | Bound history count/bytes and process indexed recovery in fixed fair slices outside population-sized scans. |
| `TP-BUDGET-001` Continuous hostile-state accounting | `tx-pool/src/component/pre_pool/mod.rs`<br>`tx-pool/src/component/pre_pool/runtime.rs`<br>`tx-pool/src/authority/resources.rs`<br>`tx-pool/src/authority/plan.rs`<br>`tx-pool/src/authority/runtime.rs`<br>`tx-pool/src/service/effects.rs`<br>`tx-pool/src/service/message.rs`<br>`tx-pool/src/service/controller.rs`<br>`tx-pool/src/metrics.rs`<br>`sync/src/relayer/block_proposal_process.rs`<br>`util/constant/src/sync.rs`<br>`util/metrics/src/lib.rs` | Global and per-peer count, bytes and active-work budgets continuously charge payload and conservative metadata in every resident state, including bounded terminal effects. Each move-only compute capability reserves exact transient bytes and edges from the same configured physical envelope; checkout and settlement exchange that reservation atomically with retained ownership. Only a count-and-byte validated relay batch may enter the bounded dispatcher channel. Operational gauges are read-only snapshots of the same maintained counters and publish only after authority locks are released. | Parking, invalidation, reservation, peer churn, excessive worker configuration, an oversized relay batch or an oversized displacement plan must not refund resident state, multiply the configured physical ceiling, start with grants incapable of holding one entry, retain attacker-sized channel payload, evict unrelated stronger work or mutate before proving the bound. | T1, T2, T3, T4, T5, T6, T7, T8, T10, T11, T13 | - Is every owner charged if and only if it is resident?<br>- Are retained bytes/edges and every simultaneous compute grant charged under one physical ceiling before mutation?<br>- Does runtime assembly reject a worker/budget ratio whose per-capability grant cannot hold one weighted entry?<br>- Does an impossible peer admission fail before global eviction planning?<br>- Can a raw transaction Vec cross the dispatcher boundary without the relayer's count and byte proof?<br>- Can metrics influence admission or require a scan, dynamic label, extra authority lock or retained payload? | Budget checks and victim selection use maintained bounded indexes; no attacker-sized repair on the admission hot path. |
| `TP-WORKER-001` Level-triggered executable readiness | `tx-pool/src/component/pre_pool/queue.rs`<br>`tx-pool/src/component/pre_pool/runtime.rs`<br>`tx-pool/src/service/workers.rs`<br>`tx-pool/src/service/builder.rs`<br>`tx-pool/src/service/stages/runner.rs`<br>`tx-pool/src/authority/state.rs`<br>`tx-pool/src/authority/work.rs`<br>`tx-pool/src/authority/plan.rs` | Readiness is derived after each transition from the authoritative capability-aware checkout predicate; internal job computation returns a typed settlement and re-arms retained work, while loss of worker/command authority requests controlled service shutdown rather than panic recovery or model restart. A successful Resolve or Verify completion may check out only the next same-lane lease inside the same kernel mutation; VerifyLease carries both the checkout capability and the exact remote declared-cycle or trusted payload policy, so completion cannot widen either. Every negative verification result is bound to both the checked-out chain view and payload policy. Apply compares that validity domain with the current owner: an unchanged policy terminalizes, while a racing same-witness trusted promotion retains the resolved payload and requeues Verify. The applied-completion type keeps a later checkout fault distinct from completion failure, and pause, cancellation or command loss completes at most that already-owned continuation without checking out another. | A capped peer backlog must not self-wake into mutex-starving livelock; a small-only worker must not consume the only wake for large work; a same-witness Proposal racing either a remote cycle-ceiling verification failure or a declared-cycle mismatch must not publish stale blame or lose its trusted payload; cancellation, zero workers, a consumed Notify around a typed job failure or a permanently closed command channel must neither strand executable work, drop an already checked-out continuation nor create a respawn loop. | T1, T2, T3, T4, T5, T7, T8, T10, T11, T13 | - Does notification mean at least one worker of this capability can execute now?<br>- Can a consumed wake be reconstructed from authoritative state after subscribe or a typed job failure?<br>- Does any internal worker use panic plus catch_unwind as control flow instead of a typed result?<br>- Can a completion continuation cross a stage/lane, hide whether Apply succeeded, or bypass the capability-aware fair queue?<br>- Does a Verify lease seal its exact payload policy, and can Apply distinguish a current peer claim from one superseded by a trusted promotion?<br>- Does pause, cancellation or command loss finish at most the already checked-out continuation and prevent another checkout? | Readiness checks inspect bounded owner heads/caps only; successful completion reuses one kernel acquisition for a same-lane checkout; worker count is configuration-bounded; no restart generation, polling loop, per-item task or queue-wide scan. |
| `TP-ADMIN-001` Administrative and hostile-peer terminalization | `tx-pool/src/service.rs`<br>`tx-pool/src/service/pipeline_ops.rs`<br>`tx-pool/src/process/classify.rs`<br>`tx-pool/src/component/recent_reject.rs`<br>`tx-pool/src/process/post_process.rs`<br>`tx-pool/src/process/submit/mod.rs`<br>`tx-pool/src/process/submit/rbf_commit.rs`<br>`tx-pool/src/component/pre_pool/commit.rs`<br>`tx-pool/src/component/pre_pool/lifecycle.rs`<br>`tx-pool/src/service/stages/verify.rs`<br>`tx-pool/src/authority/ban.rs`<br>`tx-pool/src/authority/effect.rs`<br>`tx-pool/src/authority/publisher.rs`<br>`tx-pool/src/authority/rejection.rs`<br>`tx-pool/src/authority/state.rs`<br>`tx-pool/src/authority/work.rs`<br>`tx-pool/src/authority/plan.rs`<br>`sync/src/relayer/mod.rs`<br>`sync/src/types/mod.rs` | Ban, expiry and malformed-input policy terminalize the current non-committing owner once, release all resource/index state and publish only policy-eligible effects. A remote declared-cycle claim belongs to the exact witness payload and current peer policy: a current mismatch terminalizes with immutable ingress and payload-blame attribution, while trusted same-witness promotion removes payload blame and makes an in-flight mismatch stale. UAK peer revocation is one authority Plan/Apply that always installs an expiring, non-evicting peer marker, even when its indexed cohort is empty, and removes the complete bounded PreAccepted ingress cohort including Ready and checked-out work. Accepted membership is outside that cohort, while late work settlement becomes stale. The same Apply appends one cardinality-independent exact critical PeerCohortRevoked effect. Malformed evidence seals the culprit hash and bounded public rejection; its publisher records that exact recent reject, bans only for the remaining duration of the committed marker lease and requires one relayer GenerationReset. Effect Full applies nothing, generation replacement preserves the peer marker, and the reset releases stale known/pending projections without a transaction tombstone, so another non-banned peer may submit the same transaction. | An already queued controller message, empty resident cohort, active/Ready/source-promoted malicious-peer owner, concurrent Accepted commit, saturated effect journal, delayed publisher or generation clear must not cross the ban fence, shorten or erase it, roll back Accepted state, leak a budget/index/lease ghost, resurrect through late work, silently lose the network ban/recent reject, restart a full ban duration, pin the transaction hash globally or turn a typed malformed input into fail-stop. | T1, T2, T3, T4, T7, T8, T10, T11, T13 | - Does administrative removal make every outstanding lease stale atomically?<br>- Are mutable scheduling source and immutable ingress attribution modeled separately, with every derived index and budget cleaned from the authoritative removal plan?<br>- Does VerifyLeased-to-Ready derive its scheduling source inside the same revision transition, without a check-then-publish read?<br>- Can a declared-cycle mismatch publish peer blame only while the exact checked-out payload policy is still peer-authoritative?<br>- Does Ready selection return a non-copyable session whose exclusive kernel borrow survives until accepted or rejected Plan/Apply?<br>- Does the authority commit the peer marker even when no owner is currently indexed, so a pre-ban queued ingress message cannot pass later?<br>- Does the same Apply remove the complete indexed PreAccepted cohort, including active and Ready owners, while never selecting Accepted membership?<br>- Is peer cleanup represented by one exact constant-cardinality effect whose backpressure leaves both marker and owners unchanged?<br>- Can only an authority-committed PeerCohortRevoked effect create a network ban, with no publisher-side policy inference from a generic rejection?<br>- Are the malformed culprit's recent reject and network ban retained as non-rebuildable effect detail while only relayer filters use GenerationReset?<br>- Does delayed publication consume the committed marker's remaining lease instead of starting a new full ban, and does clear preserve that marker?<br>- Can the same transaction be admitted immediately from another peer without a hash tombstone? | Ordinary Remote admission adds one O(1) peer-marker lookup under the existing authority guard. The rare ban Apply visits only the per-peer indexed cohort and affected dependency edges, both bounded by charged per-peer residency, emits one effect independent of cohort size, and prunes a monotonic expiration queue in amortized O(expired markers) without scanning live peers, Accepted or the unrelated transaction population; the required global relayer reset is an explicit rare-path availability trade-off. |
| `TP-EFFECT-001` Statically partitioned stable-state effects | `tx-pool/src/service/effects.rs`<br>`tx-pool/src/service/builder.rs`<br>`tx-pool/src/callback.rs`<br>`tx-pool/src/authority/effect.rs`<br>`tx-pool/src/authority/publisher.rs`<br>`tx-pool/src/authority/plan.rs`<br>`tx-pool/src/authority/runtime.rs` | An ordinary mutation-coupled immutable batch passes one exact source-derived region predicate, runs a total Apply and enters the global sequence under the journal's innermost lock; Full is mutation-free and no reservation or capacity token crosses a lock or await. Authority-dependent duplicate success holds an accepted-membership read capability through append. Remote, trusted and chain-critical regions are isolated, resident queued/active records are exactly charged, and only chain convergence that cannot publish per-item detail installs one prebuilt replaceable GenerationReset without waiting behind the FIFO. Chain status is rebuildable while collapsed per-item callback/recent-reject detail is an explicit observational trade-off; peer revocation and malformed-culprit evidence use exact critical detail and cannot enter that fallback. Every recordable rejection also owns one charged raw-hash projection into its resident immutable batch, versioned by both Apply sequence and effect position; public conversion occurs after the authority guard opens, and completing an older position or sequence cannot erase a newer pending RPC result. Effect checkout, exact settlement, close and closed-and-drained observation pass through the AuthorityRuntime facade; every successful Apply retires outside the guard before publishing one level hint. The sole publisher consumes only the committed outcome and a typed endpoint-step cursor: cancellation commits completed endpoint progress back to the same effect authority, so recheckout resumes at the first unacknowledged endpoint without rereading lifecycle state. | Remote saturation, a full critical FIFO, repeated rejection of one raw hash across or within batches, a permanently full or disconnected relay endpoint, callback/network/database re-entry/failure/hang, cancellation after an earlier endpoint completed, accepted duplicate racing clear/reorg, a concurrent publisher, close race or check-before-wait window must not mutate without a matching authority record, erase a newer pending rejection, replay a completed callback/ban/database write, consume trusted/critical progress, delay chain convergence, expose intermediate state, reorder a retained batch, lose charge, spawn unbounded workers, fail-stop the service or sleep forever. | T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13 | - Is every ordinary immutable batch exact and assigned from the selected owner's source before Apply?<br>- Does Full/Closed/Oversize return before Apply, with the journal lock innermost and no capacity token crossing a lock or await?<br>- Are Remote, trusted and critical usage accounted independently while queued and active, and can only saturated chain-convergence detail fall back to the prebuilt reset register rather than wait?<br>- Does an idle publisher subscribe before checkout, and can close wake it while queued, active and retained leases still drain in total order?<br>- Does the pending recent-reject projection select and retire the exact latest sequence plus in-batch effect position while keeping serialization outside the authority guard?<br>- Does every potentially blocking foreign endpoint have a bounded timeout/circuit, and does every authority-dependent acknowledgement retain its read/write capability until journal append?<br>- Does cancellation retain a typed endpoint-step cursor in the sole effect authority, retrying at most the currently unacknowledged endpoint rather than replaying completed earlier endpoints? | One bounded journal publisher, one bounded callback endpoint worker, four fixed endpoint steps per semantic outcome and at most one timed-out detached call per blocking endpoint circuit; endpoint progress is local while I/O runs and takes no authority lock until completion or cancellation retention. Recordable rejection reads use one O(1) charged hash lookup and clone only an Arc under the guard. Admission/accounting remains O(1), with no per-effect retry task, unbounded channel, callback-under-lock or busy retry. |
| `TP-REORG-001` Serialized reliable chain transitions | `tx-pool/src/process/reorg.rs`<br>`tx-pool/src/component/pre_pool/recovery.rs`<br>`tx-pool/src/service.rs`<br>`tx-pool/src/service/pipeline_ops.rs`<br>`tx-pool/src/authority/chain.rs`<br>`tx-pool/src/authority/chain_boundary.rs`<br>`tx-pool/src/authority/plan/chain_transition.rs`<br>`tx-pool/src/authority/runtime.rs`<br>`tx-pool/src/authority/state.rs` | One chain-authoritative reorg phase switches snapshot/membership once, combines detached transactions with the complete bounded accepted descendant closure of detached producers, installs one charged parent-first Recovery-source plan in the ordinary six-state kernel, carries the canonical conflicting outpoint into every chain-conflict terminal effect, and journals the immediate reset plus level-triggered template generations. | A late accepted child, startup zombie reconcile, over-budget or over-fanout descendant closure, duplicate/colliding detached roots, several conflict cells reaching one owner, clear/save races or a stale/failed full-template rebuild must not replay chain mutation, expose a parentless accepted suffix or unowned payload interval, lose or nondeterministically select the public dead-outpoint reason, retain transaction-bearing blocks outside the kernel, cross a missing parent prefix or publish stale uncles/template state. | T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13 | - Does the paired mutation follow TxPool then PrePoolKernel then innermost EffectJournal, with every recovery payload charged before locks open and no stateful guard crossing an await?<br>- Can any derived retry replay the authoritative chain mutation or retain full detached blocks as a second payload owner?<br>- Are accepted descendants planned before startup zombie reconciliation, retained parent-first, and reset as one ephemeral generation when the complete bounded closure cannot fit?<br>- Are duplicate/collision handling, full/witness identity, non-reused versions and clear epoch ordering exact?<br>- Does chain-conflict evidence remain a required typed field from causal selection through the committed effect, with a deterministic join for multiple cells? | The single graph traversal/transfer stops at fixed mutation and recovery bounds and uses one generation reset beyond them; no recovery lock, replay worker, duplicate accepted traversal or full-block payload owner remains. |
| `TP-PERSIST-001` Coherent persistence recovery point | `tx-pool/src/service.rs`<br>`tx-pool/src/persisted.rs`<br>`tx-pool/src/component/pre_pool/recovery.rs`<br>`tx-pool/src/component/pool_map.rs`<br>`tx-pool/src/process/reorg.rs` | Under one TxPool-read then PrePoolKernel snapshot boundary, persistence copies causal-parent-first accepted entries plus every retained/leased recovery-source payload into a bounded v2 envelope, releases authority locks, and serializes atomic writers without draining live state; startup also accepts the legacy v1 vector and revalidates every transaction. | Save racing detached replay/clear, an active recovery lease, malformed or oversized disk data, writer failure or expanded dep-group ordering must not persist an ownership gap, allocate from an unbounded length, lose the previous atomic file, drain the live pool or serialize children before required parents. | T1, T2, T3, T4, T6, T7, T8, T9, T10, T11, T12, T13 | - Does save use only the universal TxPool-read then kernel order and include accepted xor every recovery owner at one linearization point?<br>- Is PoolMap the accepted ordering authority, including expanded dependencies, while recovery metadata preserves parent-first session order?<br>- Are file bytes/counts bounded before allocation, writes temp-sync-rename atomic, and all startup payloads revalidated rather than trusted? | Shutdown-only clone/sort work stays off admission paths; do not add a continuously maintained persistence projection. |
| `TP-DEFECT-001` Rust-native failure and persistence boundary | `tx-pool/src/component/pre_pool/runtime.rs`<br>`tx-pool/src/component/pre_pool/lifecycle.rs`<br>`tx-pool/src/process/mod.rs`<br>`tx-pool/src/process/submit/mod.rs`<br>`tx-pool/src/metrics.rs`<br>`tx-pool/src/service/builder.rs`<br>`tx-pool/src/service/effects.rs`<br>`tx-pool/src/service/stages/resolve.rs`<br>`tx-pool/src/service/dispatch.rs`<br>`tx-pool/src/service/stages/verify.rs`<br>`sync/src/relayer/mod.rs`<br>`util/metrics/src/lib.rs` | Static ownership and private constructors make invalid authority states unconstructable; every legal malformed, policy, capacity, duplicate and stale outcome remains typed before Apply; a pre-Apply primary/index/accounting system fault stops without persistence and is never converted to an RPC/peer outcome. Panic plus catch_unwind is not authority, worker, retry or rollback control flow. Low-cardinality counters observe the existing typed-fault, worker-exit, handler-unwind and effect-publisher boundaries but never select them. | A reproducible legal transaction, stale lease, foreign endpoint failure/hang or relay saturation must not panic or reach a structural fault; a genuine pre-Apply system fault must not be converted to success/rejection, restarted into an unknown generation or written to persistence. | T1, T2, T3, T4, T6, T7, T8, T9, T10, T11, T12, T13 | - Can every peer-selectable legal outcome be traced to a typed result before mutation without assert, expect, unwrap, panic or unreachable?<br>- Does the type system prevent mutation between Plan and single-consumption total Apply?<br>- Has panic plus catch_unwind been removed from internal job, worker, publisher and authority protocols?<br>- Are genuinely foreign callbacks/endpoints isolated without selecting tx-pool state, retry or rollback semantics?<br>- Does a pre-Apply system fault stop supervised state work and skip persistence? | Proof-carrying plans and typed results are zero-cost ownership encodings on the healthy path; no recovery mutex, quarantine lookup, spare generation, unwind-driven retry or restart protocol is added. |
| `TP-POOL-001` Atomic accepted-pool graph integrity | `tx-pool/src/component/pool_map.rs`<br>`tx-pool/src/component/links.rs`<br>`tx-pool/src/pool.rs` | Full-hash pool entries are authoritative; proposal slots, status counters, causal links, outpoint indexes, sort keys and aggregate totals are rebuildable projections changed by one immutable bounded plan and total Apply. | Ghost links, short-ID collision, counter drift, late-parent insertion, expired-parent cascades or virtual self-eviction must not corrupt graph weights, preserve impossible children or partially remove accepted entries. | T1, T2, T3, T4, T5, T6, T8, T9, T10, T11, T12, T13 | - Does one immutable plan cover the complete mutation before any write?<br>- Does the independent audit rebuild agree after Apply and remain unchanged after every rejection?<br>- Are required parents distinguished from ordering-only references? | Mutation and audit work is bounded by explicit graph/victim caps; cold repair must never hide hot-path drift. |
| `TP-TEMPLATE-001` Block-template liveness and priority | `tx-pool/src/block_assembler`<br>`tx-pool/src/pool.rs`<br>`tx-pool/src/process/reorg.rs` | Reset and full rebuild retain one ordered publication authority without serializing their construction: a full rebuild derives uncle content from the bounded candidate authority, wins over racing partial revisions but cannot cross a newer reset epoch, while reset consumes its exact generation token. Uncle, proposal and transaction partial updates remain concurrent revision-checked OCC, and every successful full/reset replacement reissues all three optimistic dirty generations so valid recovered transactions always regain a proposal/commit path. External template scripts are observational, own at most one live child per configured command and terminate that child on timeout; HTTP notification work remains cancellable and outside authority locks. | Detached uncle proposals, stale Gap status, an update_full/partial race or stale partial acknowledgement must not make a valid transaction RPC-pending but forever absent from normal get_block_template mining, nor serialize independent partial work behind full calculation. A hung notification script must not turn repeated template updates into unbounded child processes, and a hung HTTP endpoint must not retain tx-pool authority past its configured timeout. | T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13 | - Does the co-located revision/reset epoch preserve full-over-partial priority and reset-over-stale-full authority while re-dirtying uncle/proposal/transaction generations after replacement?<br>- Does a full rebuild derive detached uncles from the candidate authority instead of copying the reset template's transient blank projection?<br>- Can uncle proposal filtering exclude the sole proposal path of a recovered transaction?<br>- Are Gap/Pending/Proposed transitions reflected to assembler selection?<br>- Do uncle/proposal/transaction calculations remain concurrent with a waiting full rebuild and publish only through version checks?<br>- Can any configured notification script accumulate live owned children in proportion to template update rate?<br>- Does HTTP notification remain cancellable and bounded by visible trusted configuration rather than transaction ownership? | Keep optimistic CAS updates and bounded selection; do not serialize every delta behind full rebuild or remove bounded packing safeguards without measurements. |
| `TP-IDENTITY-001` Full transaction and witness identity | `tx-pool/src/process/submit/mod.rs`<br>`tx-pool/src/component/pre_pool/mod.rs`<br>`tx-pool/src/component/pool_map.rs` | Ownership and duplicate boundaries use full raw hashes, proposal short IDs remain non-authoritative indexes, and verification-cache proofs are keyed only by the exact witness hash through TxVerificationCacheKey. | Short-ID collisions or same-raw/different-witness variants must not alias accepted/cache/history ownership, obtain a false duplicate success or reuse an invalid verification proof during reorg. | T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13 | - Is a short ID used only where collision-aware lookup semantics are explicit?<br>- Can any cache call construct a key from raw hash or arbitrary bytes?<br>- Does reorg recovery query the exact transaction witness variant? | Use compact full hashes/typed cache keys without retaining packed backing; collision handling remains indexed and bounded. |
| `TP-PERF-001` Bounded attacker-controlled work | `tx-pool/src/component/pre_pool/queue.rs`<br>`tx-pool/src/component/pre_pool/lifecycle.rs`<br>`tx-pool/src/component/pre_pool/commit.rs`<br>`tx-pool/src/service/stages/runner.rs`<br>`tx-pool/src/benchmark.rs`<br>`tx-pool/scripts/benchmark.py`<br>`tx-pool/scripts/profile.py`<br>`tx-pool/docs/PERFORMANCE.md` | Owner-head scheduling, victim selection, conflict probing and dependency publication stop at maintained bounds independent of unrelated population. Each owner's verify work is stored in disjoint cycle-class partitions so a small-only head never scans a large-cycle prefix, while Any preserves the original total order across both partitions. Cohort dependency publication reduces requested waiter deltas once against the immutable Plan and scans the smaller of requested and observed key sets without adding a resident index. Successful Resolve and Verify completion can reuse its kernel acquisition for one capability-compatible same-lane checkout without crossing a stage or adding scheduler state. Benchmark comparisons use fingerprinted paired revisions and reject noisy samples; profiling uses the same fixture and a manifest-bound submission-to-stable-callback window. | A capped peer prefix, a single owner with a large-cycle backlog, a multi-parent fan-out, stronger suffix or large independent population must not turn one admission/checkout/cohort publication into an O(pool) or Cartesian-product scan; a noisy, mismatched or whole-process-only harness must not claim a performance win or attribute fixture work to target transactions. | T1, T2, T3, T4, T6, T7, T8, T10, T11, T13 | - Is operation count bounded by owners/cohort/config rather than resident transaction count?<br>- Does capability filtering use disjoint canonical membership or another statically bounded lookup instead of rescanning one owner's queue?<br>- Does cohort publication reduce changed edges once and avoid multiplying dependency keys by cohort members?<br>- Did the change add allocation, lock, task, scan or mutable projection to a hot path, or can an existing acquisition safely carry the next same-lane lease?<br>- Before accepting benchmark evidence, are revision, binary, config, repetitions and spread comparable?<br>- Does profiling crop to the emitted target window, verify its raw/symbol artifact hashes and keep sample residency separate from CPU intervals? | Small-only owner-head lookup is independent of that owner's large-cycle population; dependency publication is O(sum of the smaller requested/observed key frontier per changed primary), not O(changed keys × cohort members). Deterministic operation-count regressions are always required; controlled timing A/B is a separate final gate, never inferred from unit duration. |

### Executable evidence

#### `TP-OWN-001` — Single pre-pool ownership

Minimum command: `cargo nextest run -p ckb-tx-pool --lib -E 'test(/(concrete_kernel_transitions|stale_lease_cannot_mutate|randomized_public_transitions|target_model_generated_commands|uak_query_never_splices|uak_read_view_keeps)/)'`

Rust evidence:

- `concrete_kernel_transitions_preserve_recomputed_projections` (T1, T2, T3, T4, T5, T6, T7, T8, T11)
- `randomized_public_transitions_always_match_full_rebuild` (T1, T2, T3, T4, T5, T6, T7, T8, T10, T11, T13)
- `stale_lease_cannot_mutate_a_removed_and_readmitted_hash` (T1, T2, T3, T4, T7, T11)
- `target_model_declares_exactly_the_frozen_six_states` (T1, T2, T3)
- `target_model_generated_commands_preserve_partition_lease_budget_and_indexes` (T1, T2, T3, T4, T5, T6, T7, T8, T11)
- `target_model_stale_lease_cannot_mutate_a_replaced_witness_owner` (T1, T2, T3, T4, T7, T11)
- `uak_active_trusted_witness_replacement_atomically_stales_obsolete_work` (T1, T2, T3, T4, T7, T8, T11)
- `uak_query_never_splices_two_authority_cuts` (T1, T2, T3, T6, T9, T10, T12, T13)
- `uak_read_view_keeps_unaccepted_payloads_visible_without_fabricating_proof` (T1, T2, T3, T4, T9, T10, T12, T13)

Process-level evidence:

- `local-test-rpc-direct`: `test/src/specs/tx_pool/local_test_submission.rs::LocalTestSubmissionIsDirect` (T1, T2, T3, T4, T6, T7, T11) — The integration-only send_test_transaction RPC returns a typed missing-parent rejection without stopping the service, then synchronously commits a valid Local transaction with no pre-pool owner or verify-queue residue. Paired units: `local_submit_bypasses_and_settles_matching_remote_owner`. Command: `make integration CKB_TEST_ARGS='-c 1 LocalTestSubmissionIsDirect'`

#### `TP-COMMIT-001` — Authoritative commit and handoff

Minimum command: `cargo nextest run -p ckb-tx-pool --features internal --lib -E 'test(/(verified_commit|pipeline_commit_worker|local_submit_bypasses|target_model_exercises_every_plan_outcome|sparse_plan_matches|accepted_duplicate_relay|resolution_evidence|uak_direct_local|uak_dropped_direct_local|uak_final_admission|uak_final_validation|uak_changed_tip_revalidates|uak_same_tip_unproven|uak_pool_origin_refresh|uak_script_rule_change|uak_matching_completion)/)'`

Rust evidence:

- `equal_priority_ready_candidates_commit_earlier_arrival_first` (T1, T2, T4, T6, T7, T11)
- `local_submit_bypasses_and_settles_matching_remote_owner` (T1, T2, T3, T4, T6, T7, T11)
- `pipeline_commit_worker_waits_for_the_pool_sequencer` (T5, T6)
- `target_model_exercises_every_plan_outcome_without_partial_mutation` (T1, T2, T4, T6, T7, T11)
- `tx_pool_resolution_evidence_requires_chain_provenance` (T6, T8)
- `tx_pool_resolution_evidence_yields_to_pool_spends_and_tip_changes` (T6, T9)
- `tx_pool_same_tip_resolution_evidence_skips_chain_revalidation` (T6, T8)
- `uak_changed_tip_revalidates_header_dependencies` (T2, T4, T6, T7, T9)
- `uak_direct_local_admission_moves_from_absent_to_accepted_in_one_apply` (T1, T2, T3, T6, T7)
- `uak_direct_local_atomically_stales_the_matching_remote_compute_capability` (T1, T2, T3, T6, T11)
- `uak_direct_local_duplicate_commits_an_outcome_without_owner_mutation` (T1, T2, T6, T7)
- `uak_direct_local_replaces_inactive_remote_payload_without_losing_attribution` (T1, T2, T3, T6, T7)
- `uak_dropped_direct_local_plan_is_semantically_mutation_free` (T1, T2, T3, T6)
- `uak_final_admission_refreshes_stale_verification_context` (T6, T9)
- `uak_final_admission_rejects_a_changed_validation_ruleset` (T6, T9)
- `uak_final_validation_rejects_a_mixed_authority_snapshot_cut` (T2, T6, T9)
- `uak_final_validation_reuses_same_tip_positive_location_evidence` (T2, T6, T8)
- `uak_matching_completion_settles_and_refreshes_across_chain_view_change` (T1, T2, T3, T6, T9, T11)
- `uak_pool_origin_refresh_is_coupled_and_retires_the_old_payload_outside_apply` (T1, T2, T3, T4, T6)
- `uak_same_tip_unproven_location_is_rejected_not_treated_as_pool_origin` (T2, T6, T8)
- `uak_script_rule_change_requeues_the_exact_owner_for_resolution` (T2, T6, T9, T11)
- `verified_commit_effect_backpressure_falls_back_to_ready` (T1, T2, T3, T5, T6, T7, T10, T11)
- `verified_commit_session_commits_the_canonical_leased_owner_without_ready_publication` (T1, T2, T3, T5, T6, T7, T11)
- `verified_commit_session_defers_to_a_stronger_ready_owner` (T1, T2, T5, T6, T11)

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
- `uak_direct_local_under_fee_rbf_rejects_without_touching_any_owner` (T1, T3, T5, T6, T7)
- `verified_commit_session_uses_the_existing_ready_conflict_cohort` (T1, T2, T4, T5, T6, T7, T11)

Process-level evidence:

- `rbf-basic`: `test/src/specs/tx_pool/replace.rs::RbfBasic` (T1, T2, T4, T5, T6, T7, T11) — The node enforces the replacement fee rule, exposes the victim as RBFRejected, commits the accepted higher-fee replacement, and does not reinterpret an output created and consumed in that attached branch as an availability edge that overwrites the root rejection. Paired units: `successful_replacement_does_not_recover_removed_descendants`, `dependency_availability_uses_the_authoritative_overlay_level`, `uak_rbf_replaces_the_complete_descendant_closure_atomically`, `uak_failed_rbf_fee_disposition_preserves_victims_and_terminalizes_candidate`. Command: `make integration CKB_TEST_ARGS='-c 1 RbfBasic'`
- `rbf-cell-deps`: `test/src/specs/tx_pool/replace.rs::RbfCellDepsCheck` (T5, T6) — Final node admission rejects a replacement that depends on a cell from its victim closure and records the failed candidate without corrupting the accepted pool graph. Paired units: `rbf_rejects_dep_group_member_from_replacement_victim`. Command: `make integration CKB_TEST_ARGS='-c 1 RbfCellDepsCheck'`
- `rbf-concurrency`: `test/src/specs/tx_pool/replace.rs::RbfConcurrency` (T5, T6) — Concurrent RPC submissions for one input converge to the unique highest-fee transaction and record every losing candidate as rejected without a recovery livelock. Paired units: `pipeline_rejects_conflicting_double_spend`, `successful_replacement_does_not_recover_removed_descendants`, `uak_failed_rbf_fee_disposition_preserves_victims_and_terminalizes_candidate`. Command: `make integration CKB_TEST_ARGS='-c 1 RbfConcurrency'`
- `rbf-conflict-recovery`: `test/src/specs/tx_pool/orphan_tx_recovery.rs::RbfOrphanRecovery` (T1, T2, T4, T5, T6, T7, T11) — When a replacement frees only one of two historical inputs, the node recovers exactly the newly eligible victim and retains the still-conflicting victim as rejected. Paired units: `full_conflict_history_terminalizes_rejected_owner_without_panicking`, `successful_replacement_does_not_recover_removed_descendants`, `uak_replacement_history_survives_winner_commit_and_wakes_after_reorg`, `uak_replacement_history_wakes_only_on_newer_projected_availability`, `uak_replacement_history_waits_for_every_observed_blocker`. Command: `make integration CKB_TEST_ARGS='-c 1 RbfOrphanRecovery'`
- `rbf-cycling-attack`: `test/src/specs/tx_pool/replace.rs::RbfCyclingAttack` (T1, T2, T4, T5, T6, T7, T11) — A cyclic sequence of overlapping replacements recovers the pre-existing transaction whose input becomes free, while the victim retained by that same atomic replacement observes the post-Apply dependency cut and does not resurrect while its input remains consumed. Paired units: `same_commit_wakes_prior_conflict_but_not_its_new_victim`, `full_conflict_history_terminalizes_rejected_owner_without_panicking`, `successful_replacement_does_not_recover_removed_descendants`, `uak_replacement_history_wakes_only_on_newer_projected_availability`, `uak_rbf_unions_fan_in_descendants_once_and_removes_children_first`. Command: `make integration CKB_TEST_ARGS='-c 1 RbfCyclingAttack'`
- `rbf-proposed-success`: `test/src/specs/tx_pool/replace.rs::RbfReplaceProposedSuccess` (T5, T6, T12, T13) — A higher-fee replacement of a Proposed chain member survives a subsequent blank block, is freshly proposed, and is committed while the displaced closure remains rejected. Paired units: `successful_replacement_does_not_recover_removed_descendants`, `commit_and_removal_journal_block_assembler_delta`. Command: `make integration CKB_TEST_ARGS='-c 1 RbfReplaceProposedSuccess'`
- `rbf-proposed-template-refresh`: `test/src/specs/tx_pool/replace.rs::RbfRejectReplaceProposed` (T5, T6, T12, T13) — Replacing a Proposed transaction rejects the victim, refreshes the real node template, and normally proposes and commits only the replacement. Paired units: `successful_replacement_does_not_recover_removed_descendants`, `commit_and_removal_journal_block_assembler_delta`. Command: `make integration CKB_TEST_ARGS='-c 1 RbfRejectReplaceProposed'`

#### `TP-DEP-001` — Causal dependency graph

Minimum command: `cargo nextest run -p ckb-tx-pool --features internal --lib -E 'test(/(parent_commit_before_wait|parent_loss|successive_expanded_parent_losses|terminalized_superseded_parent|dependency_epochs|dep_reader|selected_reader|conditional_cycle|dep_group|unknown_outpoint|uak_remote_missing|uak_direct_parent_acceptance|uak_chain_tip_not_revision|uak_coupled_membership_requires|uak_template_orders_selected|uak_template_sheds)/)'`

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
- `uak_chain_tip_not_revision_controls_negative_evidence_freshness` (T2, T4, T9, T11)
- `uak_chain_trusted_proposal_expiry_publishes_definitive_parent_loss` (T1, T2, T4, T7, T9, T11, T13)
- `uak_coupled_membership_requires_exact_positive_input_evidence` (T1, T2, T4, T6, T9, T11)
- `uak_direct_parent_acceptance_publishes_output_availability_atomically` (T1, T2, T3, T4, T6, T7, T11)
- `uak_remote_missing_wait_and_parent_request_share_one_backpressured_apply` (T1, T2, T3, T4, T7, T8, T10, T11, T13)
- `uak_template_orders_selected_dependency_reader_before_spender` (T4, T11, T12, T13)
- `uak_template_sheds_conditional_cycles_deterministically` (T3, T4, T8, T10, T11, T12, T13)

Process-level evidence:

- `accepted-descendants-after-parent-reorg`: `test/src/specs/tx_pool/pool_reconcile.rs::PoolResolveConflictAfterReorg` (T1, T2, T3, T4, T6, T9, T10, T11, T12, T13) — When a formerly committed parent is detached while its child and grandchild remain accepted, the node transfers the complete accepted closure into parent-first recovery, restores all three as Proposed and retains the ordinary dead-input conflict verdict. Paired units: `accepted_reorg_recovery_plan_is_parent_first_and_total`, `reorg_replays_detached_parent_with_accepted_descendant_closure`, `over_bound_reorg_descendant_closure_resets_ephemeral_pool_generation`. Command: `make integration CKB_TEST_ARGS='-c 1 PoolResolveConflictAfterReorg'`
- `cell-dep-arrival-order`: `test/src/specs/tx_pool/dead_cell_deps.rs::CellBeingCellDepAndSpentInSameBlockTestGetBlockTemplate` (T4, T6, T11, T12, T13) — Both reader-first and spender-first RPC arrival orders retain the valid pair; normal get_block_template deterministically places the cell-dep reader before the spender and commits both. Paired units: `pipeline_accepts_dep_reader_after_in_flight_spender`, `selected_reader_is_ordered_before_spender`, `conditional_cycle_drops_weakest_member`. Command: `make integration CKB_TEST_ARGS='-c 1 CellBeingCellDepAndSpentInSameBlockTestGetBlockTemplate'`
- `conditional-reader-fanout`: `test/src/specs/tx_pool/limit.rs::TxPoolLimitAncestorCount` (T3, T6, T8, T10, T11, T13) — Two thousand cell-dep readers coexist with a spender without becoming persistent ancestors, while a genuine causal chain still stops at the configured ancestor limit. Paired units: `popular_dep_readers_coexist_with_spender`, `dep_readers_do_not_count_as_spender_ancestors`, `causal_ancestor_limit_never_evicts_existing_entries`. Command: `make integration CKB_TEST_ARGS='-c 1 TxPoolLimitAncestorCount'`
- `dependent-reorg-chain`: `test/src/specs/tx_pool/reorg_recovers_dependent.rs::ReorgRecoversDependentChain` (T4, T9, T10, T11, T12, T13) — A three-level detached dependency chain is replayed parent-first and every member returns to a committable pool state after the node adopts the competing fork. Paired units: `sort_txs_by_dependencies_orders_parents_before_children`, `reorg_direct_replay_treats_pool_duplicates_as_idempotent`. Command: `make integration CKB_TEST_ARGS='-c 1 ReorgRecoversDependentChain'`
- `dependent-reorg-transactions`: `test/src/specs/tx_pool/reorg_recovers_dependent.rs::ReorgRecoversDependentTxs` (T4, T9, T10, T11, T12, T13) — A detached parent and its dependent child are both recovered through the node reorg callback and remain committable as one dependency-ordered pool graph. Paired units: `sort_txs_by_dependencies_orders_parents_before_children`, `reorg_direct_replay_treats_pool_duplicates_as_idempotent`. Command: `make integration CKB_TEST_ARGS='-c 1 ReorgRecoversDependentTxs'`
- `multi-parent-orphan-frontier`: `test/src/specs/tx_pool/orphan_tx.rs::TxPoolOrphanReverse` (T1, T2, T4, T7, T10, T11) — A child with three unavailable direct parents installs one exact Wait owner and atomically requests the complete currently missing parent frontier, so RelayV3 accepts every reverse-order parent and the whole graph converges without polling or an inferred orphan store. Paired units: `resolve_job_registers_complete_unknown_outpoint_frontier`, `remote_parent_wait_and_unknown_parents_effect_are_one_transition`, `uak_remote_missing_wait_and_parent_request_share_one_backpressured_apply`, `journal_sequence_is_total_apply_order`. Command: `make integration CKB_TEST_ARGS='-c 1 TxPoolOrphanReverse'`
- `same-lane-relay-continuation`: `test/src/specs/tx_pool/txs_relay_order.rs::TxsRelayOrder` (T1, T2, T3, T4, T7, T8, T10, T11, T13) — Ten dependent transactions submitted to one node traverse RelayV3 and the receiving node's pre-pool Resolve/Verify workers; after relay, every transaction remains observable in exactly one pending-or-orphan pool class, while unit evidence pins same-lane checkout, worker capability, and cancellation semantics. Paired units: `successful_stage_completion_checks_out_same_lane_without_projection_drift`, `verify_continuation_uses_the_checked_out_worker_capability`, `cancellation_finishes_one_checked_out_continuation_without_dropping_it`. Command: `make integration CKB_TEST_ARGS='-c 1 TxsRelayOrder'`

#### `TP-CACHE-001` — Bounded conflict-history ownership and wakeup

Minimum command: `cargo nextest run -p ckb-tx-pool --lib -E 'test(/(full_conflict_history|same_commit_wakes_prior_conflict|repeated_dependency_epochs|availability_without_a_wait|dependency_availability_uses_the_authoritative_overlay|remote_conflict_history|remote_peer_order|pipeline_rejects_conflicting_double_spend|uak_rbf_replaces_the_complete|uak_failed_rbf_fee_disposition|uak_replacement_history|uak_rbf_unions_fan_in)/)'`

Rust evidence:

- `availability_without_a_wait_owner_retains_no_epoch_history` (T3, T8, T10, T11, T13)
- `dependency_availability_uses_the_authoritative_overlay_level` (T1, T2, T4, T5, T6, T7, T11)
- `full_conflict_history_terminalizes_rejected_owner_without_panicking` (T1, T2, T3, T4, T5, T6, T7, T8, T11)
- `remote_conflict_history_keeps_its_bounded_residency_deadline` (T1, T2, T3, T4, T7, T8, T11)
- `remote_peer_order_cannot_hijack_an_existing_conflict_owner` (T1, T2, T3, T5, T6)
- `same_commit_wakes_prior_conflict_but_not_its_new_victim` (T1, T2, T4, T5, T6, T7, T11)
- `uak_failed_rbf_fee_disposition_preserves_victims_and_terminalizes_candidate` (T1, T3, T5, T6, T7, T8)
- `uak_rbf_replaces_the_complete_descendant_closure_atomically` (T1, T3, T4, T5, T6, T7, T8, T11, T12)
- `uak_rbf_unions_fan_in_descendants_once_and_removes_children_first` (T1, T3, T4, T5, T6, T7, T8, T11)
- `uak_replacement_history_observes_only_finally_unavailable_dependencies` (T1, T2, T3, T4, T6, T7, T8, T11)
- `uak_replacement_history_requires_trusted_proposal_to_promote` (T1, T2, T3, T4, T6, T11)
- `uak_replacement_history_survives_winner_commit_and_wakes_after_reorg` (T1, T2, T3, T4, T6, T9, T11)
- `uak_replacement_history_waits_for_every_observed_blocker` (T1, T2, T3, T4, T6, T7, T8, T11)
- `uak_replacement_history_wakes_only_on_newer_projected_availability` (T1, T2, T3, T4, T6, T7, T11)

Process-level evidence:

- `rbf-basic`: `test/src/specs/tx_pool/replace.rs::RbfBasic` (T1, T2, T4, T5, T6, T7, T11) — The node enforces the replacement fee rule, exposes the victim as RBFRejected, commits the accepted higher-fee replacement, and does not reinterpret an output created and consumed in that attached branch as an availability edge that overwrites the root rejection. Paired units: `successful_replacement_does_not_recover_removed_descendants`, `dependency_availability_uses_the_authoritative_overlay_level`, `uak_rbf_replaces_the_complete_descendant_closure_atomically`, `uak_failed_rbf_fee_disposition_preserves_victims_and_terminalizes_candidate`. Command: `make integration CKB_TEST_ARGS='-c 1 RbfBasic'`
- `rbf-concurrency`: `test/src/specs/tx_pool/replace.rs::RbfConcurrency` (T5, T6) — Concurrent RPC submissions for one input converge to the unique highest-fee transaction and record every losing candidate as rejected without a recovery livelock. Paired units: `pipeline_rejects_conflicting_double_spend`, `successful_replacement_does_not_recover_removed_descendants`, `uak_failed_rbf_fee_disposition_preserves_victims_and_terminalizes_candidate`. Command: `make integration CKB_TEST_ARGS='-c 1 RbfConcurrency'`
- `rbf-conflict-recovery`: `test/src/specs/tx_pool/orphan_tx_recovery.rs::RbfOrphanRecovery` (T1, T2, T4, T5, T6, T7, T11) — When a replacement frees only one of two historical inputs, the node recovers exactly the newly eligible victim and retains the still-conflicting victim as rejected. Paired units: `full_conflict_history_terminalizes_rejected_owner_without_panicking`, `successful_replacement_does_not_recover_removed_descendants`, `uak_replacement_history_survives_winner_commit_and_wakes_after_reorg`, `uak_replacement_history_wakes_only_on_newer_projected_availability`, `uak_replacement_history_waits_for_every_observed_blocker`. Command: `make integration CKB_TEST_ARGS='-c 1 RbfOrphanRecovery'`
- `rbf-cycling-attack`: `test/src/specs/tx_pool/replace.rs::RbfCyclingAttack` (T1, T2, T4, T5, T6, T7, T11) — A cyclic sequence of overlapping replacements recovers the pre-existing transaction whose input becomes free, while the victim retained by that same atomic replacement observes the post-Apply dependency cut and does not resurrect while its input remains consumed. Paired units: `same_commit_wakes_prior_conflict_but_not_its_new_victim`, `full_conflict_history_terminalizes_rejected_owner_without_panicking`, `successful_replacement_does_not_recover_removed_descendants`, `uak_replacement_history_wakes_only_on_newer_projected_availability`, `uak_rbf_unions_fan_in_descendants_once_and_removes_children_first`. Command: `make integration CKB_TEST_ARGS='-c 1 RbfCyclingAttack'`

#### `TP-BUDGET-001` — Continuous hostile-state accounting

Minimum command: `cargo nextest run -p ckb-tx-pool --features internal --lib -E 'test(/(admission_budget_failure|parent_loss_uses_the_continuous_wait_reservation|parent_projection_deduplicates|remote_conflict_keeps_remote_reservation|owner_fairness_and_active_caps|verified_candidate_compacts|conflict_closure_aborts|self_eviction_plan|multi_input_conflict_union_respects_the_global_commit_bound|notify_tx_batch_rejects|uak_independent_rbf_churn|uak_replacement_history_reserves_raw_edges|uak_checkout_attack_work_is_bounded|runtime_configuration_rejects_an_unusable_per_work_grant)/)'`

Rust evidence:

- `admission_budget_failure_leaves_primary_and_views_unchanged` (T1, T2, T3, T4, T7, T8, T11)
- `conflict_closure_aborts_at_candidate_limit` (T3, T8, T10, T11, T13)
- `journal_usage_charges_queued_and_active_batches_exactly` (T3, T8)
- `multi_input_conflict_union_respects_the_global_commit_bound` (T3, T8, T10, T11, T13)
- `notify_tx_batch_rejects_bytes_before_dispatch` (T3, T8, T10)
- `notify_tx_batch_rejects_count_before_dispatch` (T3, T8, T10)
- `parent_loss_uses_the_continuous_wait_reservation_at_a_full_budget` (T1, T2, T3, T4, T7, T8, T11)
- `parent_projection_deduplicates_cell_and_header_edges` (T3, T6, T8)
- `remote_conflict_keeps_remote_reservation_and_wakes_without_capacity_retry` (T1, T2, T3, T4, T7, T8, T10, T11, T13)
- `runtime_configuration_rejects_an_unusable_per_work_grant` (T3, T7, T8, T10, T11)
- `uak_checkout_attack_work_is_bounded_by_owner_heads_and_active_slots` (T1, T2, T3, T7, T8, T10, T11, T13)
- `uak_independent_rbf_churn_never_exceeds_replacement_history_budget` (T1, T3, T5, T6, T8)
- `uak_replacement_history_reserves_raw_edges_until_wake` (T1, T2, T3, T4, T6, T7, T8, T11)
- `verified_candidate_compacts_deps_and_pool_budget_counts_retained_inputs` (T3, T8)

#### `TP-WORKER-001` — Level-triggered executable readiness

Minimum command: `cargo nextest run -p ckb-tx-pool --lib -E 'test(/(owner_fairness_and_active_caps|expiry_batch_is_bounded|worker_|zero_verify_worker|stage_completion_checks_out_same_lane|checked_out_continuation|verify_continuation_uses|uak_stale_dependency_head|uak_stale_remote_cycle_rejection|uak_remote_verify_failure)/)'`

Rust evidence:

- `cancellation_finishes_one_checked_out_continuation_without_dropping_it` (T1, T2, T3, T4, T7, T10, T11)
- `command_close_finishes_one_checked_out_continuation_then_exits` (T1, T2, T3, T4, T7, T10, T11)
- `expiry_batch_is_bounded_without_a_ready_prefix_scan` (T1, T2, T3, T4, T7, T8, T10, T11, T13)
- `large_owner_head_does_not_hide_its_small_cycle_work` (T1, T2, T3, T4, T7, T8, T10, T11, T13)
- `panicked_state_worker_makes_shutdown_ineligible_for_persistence` (T1, T2, T3, T4, T7, T8, T10, T11, T13)
- `pause_finishes_one_checked_out_continuation_without_dropping_it` (T1, T2, T3, T4, T7, T10, T11)
- `service_cancellation_is_scoped_under_process_exit` (T1, T4, T7)
- `uak_remote_verify_failure_requeues_after_same_witness_proposal_promotion` (T1, T2, T3, T5, T7, T8, T11)
- `uak_stale_dependency_head_cannot_abort_unrelated_checkout` (T1, T2, T4, T7, T8, T10, T11, T13)
- `uak_stale_remote_cycle_rejection_requeues_after_same_witness_proposal_promotion` (T1, T2, T3, T5, T7, T8, T11)
- `verify_continuation_uses_the_checked_out_worker_capability` (T1, T2, T3, T4, T7, T8, T10, T11)
- `worker_exits_when_command_channel_dropped` (T1, T2, T3, T4, T7, T8, T10, T11, T13)
- `zero_verify_worker_config_still_runs_remote_pipeline` (T1, T2, T4, T7, T11)

Process-level evidence:

- `same-lane-relay-continuation`: `test/src/specs/tx_pool/txs_relay_order.rs::TxsRelayOrder` (T1, T2, T3, T4, T7, T8, T10, T11, T13) — Ten dependent transactions submitted to one node traverse RelayV3 and the receiving node's pre-pool Resolve/Verify workers; after relay, every transaction remains observable in exactly one pending-or-orphan pool class, while unit evidence pins same-lane checkout, worker capability, and cancellation semantics. Paired units: `successful_stage_completion_checks_out_same_lane_without_projection_drift`, `verify_continuation_uses_the_checked_out_worker_capability`, `cancellation_finishes_one_checked_out_continuation_without_dropping_it`. Command: `make integration CKB_TEST_ARGS='-c 1 TxsRelayOrder'`

#### `TP-ADMIN-001` — Administrative and hostile-peer terminalization

Minimum command: `cargo nextest run -p ckb-tx-pool --features internal -E 'test(/(banned_peer_fence_never_evicts_an_unexpired_marker|banned_peer_revokes|banned_peer_revocation_plan_uses_immutable_ingress_attribution|peer_ban_removes_promoted_ingress_and_allows_refetch|peer_ban_does_not_rollback_an_already_accepted_transaction|ready_commit_observes_ban_fence_before_acceptance|queued_remote_admission_after_ban_is_removed_and_refetchable|proposal_promotes_active_remote_owner_without_restarting_lease|malformed_remote_preflight|proposal_promoted_remote_clear|live_marker_is_never_shortened|uak_peer_revocation|uak_clear_pipeline_preserves_live_peer_revocation|uak_current_remote_cycle_rejection|uak_final_malformed_revalidation)/)'`

Rust evidence:

- `banned_peer_fence_never_evicts_an_unexpired_marker` (T1, T2, T3, T8)
- `banned_peer_revocation_plan_uses_immutable_ingress_attribution` (T1, T2, T3, T4, T7, T8, T11)
- `banned_peer_revokes_active_remote_lease_and_releases_budget` (T1, T2, T3, T4, T7, T8, T11)
- `live_marker_is_never_shortened_and_expired_markers_are_pruned_on_record` (T1, T2, T3, T7, T8, T10)
- `malformed_remote_preflight_is_banned_recorded_and_not_relayed` (T1, T2, T4, T7, T10, T11)
- `peer_ban_does_not_rollback_an_already_accepted_transaction` (T1, T2, T3, T4, T7, T10, T11)
- `peer_ban_removes_promoted_ingress_and_allows_refetch` (T1, T2, T3, T4, T7, T8, T10, T11)
- `proposal_promoted_remote_clear_uses_generation_reset_to_release_ingress_filter` (T1, T2, T3, T4, T7, T8, T10, T11, T13)
- `proposal_promotes_active_remote_owner_without_restarting_lease` (T1, T2, T3, T4, T7, T8, T11)
- `queued_remote_admission_after_ban_is_removed_and_refetchable` (T1, T2, T3, T4, T7, T8, T10, T11)
- `ready_commit_observes_ban_fence_before_acceptance` (T1, T2, T3, T4, T7, T8, T10, T11)
- `uak_clear_pipeline_preserves_live_peer_revocation` (T1, T2, T3, T7, T8, T10)
- `uak_current_remote_cycle_rejection_terminalizes_with_peer_attribution` (T1, T2, T3, T7, T8)
- `uak_final_malformed_revalidation_revokes_the_complete_peer_cohort` (T1, T2, T3, T4, T7, T8, T10, T11)
- `uak_peer_revocation_commits_one_constant_size_cohort_effect` (T1, T2, T3, T7, T8, T10, T11)
- `uak_peer_revocation_removes_active_owner_and_makes_its_lease_stale` (T1, T2, T3, T4, T7, T8, T10, T11)
- `uak_peer_revocation_removes_only_preaccepted_ingress_owners` (T1, T2, T3, T4, T7, T8, T10, T11)
- `uak_peer_revocation_without_resident_owner_still_fences_queued_ingress` (T1, T2, T3, T4, T7, T8, T10, T11)
- `uak_proposal_promotion_suspends_but_retains_the_remote_deadline` (T1, T2, T3, T4, T7, T8, T11)
- `uak_remote_expiry_is_a_bounded_derived_transition_and_allows_refetch` (T1, T2, T3, T4, T7, T8, T10, T11, T13)
- `uak_remote_expiry_removes_active_work_without_a_drain_or_prefix_expansion` (T1, T2, T3, T4, T7, T8, T10, T11, T13)

#### `TP-EFFECT-001` — Statically partitioned stable-state effects

Minimum command: `cargo nextest run -p ckb-tx-pool --features internal --lib -E 'test(/(journal_usage|journal_sequence|full_journal|ordinary_full|remote_byte_ceiling|proposal_ready_commit|critical_headroom|authoritative_apply|authoritative_generation_reset|closed_journal_rejects_generation_reset|generation_reset_register|full_relayer|hung_callback|hung_network_endpoint|accepted_duplicate_relay|replacement_publisher|close_wakes|idle_publisher|uak_remote_missing|runtime_effect|uak_effect_compiler|uak_ordinary_relay|uak_required_parent_detail|uak_relay_disconnect|uak_publisher_relay_disconnect|uak_effect_publisher_claim|uak_cancelled_publisher|uak_retained_batch|uak_cancelled_later_endpoint|uak_unregistered_callback|uak_callback_uses|uak_pending_recent_reject)/)'`

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
- `runtime_effect_close_wakes_an_idle_level_waiter` (T3, T7, T8, T10, T11, T13)
- `runtime_effect_facade_retains_and_drains_a_closed_log_in_sequence` (T1, T2, T3, T7, T8, T10, T11, T13)
- `trusted_saturation_cannot_consume_critical_headroom` (T3, T7, T8, T10, T11, T13)
- `uak_callback_uses_the_production_timeout_and_opens_one_stable_circuit` (T3, T7, T8, T10, T11)
- `uak_cancelled_later_endpoint_does_not_replay_completed_callback` (T1, T2, T3, T7, T10, T11)
- `uak_cancelled_publisher_returns_the_complete_lease_to_the_fifo_head` (T1, T2, T3, T7, T10, T11)
- `uak_effect_compiler_exhausts_conflict_cleanup_and_required_detail_variants` (T1, T4, T7, T9, T10, T11, T12)
- `uak_effect_compiler_keeps_rejection_owner_and_peer_attribution_typed` (T1, T2, T4, T7, T8, T10)
- `uak_effect_compiler_preserves_acceptance_and_chain_endpoint_semantics` (T1, T4, T7, T10, T12)
- `uak_effect_publisher_claim_is_move_only_and_exclusive` (T1, T2, T7, T10)
- `uak_ordinary_relay_saturation_publishes_reset_before_disposal` (T3, T7, T8, T10, T11, T13)
- `uak_peer_revocation_effect_backpressure_is_zero_semantic_mutation` (T1, T2, T3, T7, T8, T10, T11, T13)
- `uak_pending_recent_reject_is_an_exact_sequence_derived_projection` (T1, T2, T3, T7, T8, T10)
- `uak_pending_recent_reject_uses_effect_position_within_one_batch` (T1, T2, T3, T7, T8, T10)
- `uak_publisher_relay_disconnect_retains_the_authority_head` (T1, T3, T7, T10, T11)
- `uak_relay_disconnect_is_typed_and_does_not_claim_publication` (T3, T7, T10, T11)
- `uak_remote_expiry_effect_backpressure_is_zero_mutation` (T1, T2, T3, T7, T8, T10, T11, T13)
- `uak_required_parent_detail_never_degrades_under_relay_saturation` (T3, T4, T7, T8, T10, T11)
- `uak_retained_batch_resumes_at_its_first_unprocessed_endpoint` (T1, T2, T3, T7, T10, T11)
- `uak_unregistered_callback_is_not_dispatched_to_the_foreign_worker` (T3, T7, T8, T10)

Process-level evidence:

- `multi-parent-orphan-frontier`: `test/src/specs/tx_pool/orphan_tx.rs::TxPoolOrphanReverse` (T1, T2, T4, T7, T10, T11) — A child with three unavailable direct parents installs one exact Wait owner and atomically requests the complete currently missing parent frontier, so RelayV3 accepts every reverse-order parent and the whole graph converges without polling or an inferred orphan store. Paired units: `resolve_job_registers_complete_unknown_outpoint_frontier`, `remote_parent_wait_and_unknown_parents_effect_are_one_transition`, `uak_remote_missing_wait_and_parent_request_share_one_backpressured_apply`, `journal_sequence_is_total_apply_order`. Command: `make integration CKB_TEST_ARGS='-c 1 TxPoolOrphanReverse'`

#### `TP-REORG-001` — Serialized reliable chain transitions

Minimum command: `cargo nextest run -p ckb-tx-pool --features internal -E 'test(/(reorg_publishes|overlapping_detached|accepted_reorg_recovery|reorg_replays_detached_parent|over_bound_reorg|recovery_batch|empty_generation_recovery|failed_block_assembler|clear_during_reorg|cross_authority_query|uak_final_admission_receipt_is_stale|uak_recovery_admission_requires|uak_chain_conflict_commits_the_canonical_dead_outpoint|uak_rules_transition|uak_chain_receipt_ignores|uak_unrepresentable_recovery_set_converges|uak_runtime_chain_boundary_converges)/)'`

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
- `uak_chain_conflict_commits_the_canonical_dead_outpoint` (T1, T2, T4, T7, T9, T10, T11, T12, T13)
- `uak_chain_proposal_demotion_preserves_active_remote_compute_capability` (T1, T2, T3, T4, T7, T8, T9, T11)
- `uak_chain_proposal_outside_demotes_remote_base_and_reactivates_its_deadline` (T1, T2, T3, T4, T7, T8, T9, T10, T11, T12, T13)
- `uak_chain_receipt_ignores_unrelated_accepted_and_preaccepted_owners` (T1, T2, T3, T7, T9, T10, T11)
- `uak_final_admission_receipt_is_stale_after_chain_view_aba` (T6, T9)
- `uak_recovery_admission_requires_the_current_generation_capability` (T1, T2, T7, T9, T11)
- `uak_repeated_proposal_has_no_synthetic_source_revision` (T1, T2, T7, T9, T11)
- `uak_rules_transition_cannot_claim_monotonic_accepted_validity` (T1, T2, T5, T6, T9, T10, T12, T13)
- `uak_runtime_chain_boundary_converges_an_unrepresentable_recovery_batch` (T1, T2, T3, T4, T7, T8, T9, T10, T11, T12, T13)
- `uak_unrepresentable_recovery_set_converges_to_a_fresh_parent_first_prefix` (T1, T2, T3, T4, T7, T8, T9, T10, T11, T12, T13)

Process-level evidence:

- `accepted-descendants-after-parent-reorg`: `test/src/specs/tx_pool/pool_reconcile.rs::PoolResolveConflictAfterReorg` (T1, T2, T3, T4, T6, T9, T10, T11, T12, T13) — When a formerly committed parent is detached while its child and grandchild remain accepted, the node transfers the complete accepted closure into parent-first recovery, restores all three as Proposed and retains the ordinary dead-input conflict verdict. Paired units: `accepted_reorg_recovery_plan_is_parent_first_and_total`, `reorg_replays_detached_parent_with_accepted_descendant_closure`, `over_bound_reorg_descendant_closure_resets_ephemeral_pool_generation`. Command: `make integration CKB_TEST_ARGS='-c 1 PoolResolveConflictAfterReorg'`
- `async-uncle-candidate-publication`: `test/src/specs/mining/uncle.rs::UncleInheritFromForkUncle` (T9, T10, T12, T13) — After a forced fork, the process test waits for phase-two detached-uncle publication instead of mistaking the authoritative blank reset template for final convergence; it then consumes every eligible uncle and validates the descendant rule. Paired units: `failed_block_assembler_update_retains_dirty_generation_for_retry`, `full_reset_and_partial_priority_use_template_owned_tokens`, `full_rebuild_derives_uncles_from_candidate_authority`, `reorg_refresh_recovers_when_blank_reset_precedes_candidate_retention`, `full_rebuild_reissues_every_optimistic_delta_generation`. Command: `make integration CKB_TEST_ARGS='-c 1 UncleInheritFromForkUncle'`
- `dependent-reorg-chain`: `test/src/specs/tx_pool/reorg_recovers_dependent.rs::ReorgRecoversDependentChain` (T4, T9, T10, T11, T12, T13) — A three-level detached dependency chain is replayed parent-first and every member returns to a committable pool state after the node adopts the competing fork. Paired units: `sort_txs_by_dependencies_orders_parents_before_children`, `reorg_direct_replay_treats_pool_duplicates_as_idempotent`. Command: `make integration CKB_TEST_ARGS='-c 1 ReorgRecoversDependentChain'`
- `dependent-reorg-transactions`: `test/src/specs/tx_pool/reorg_recovers_dependent.rs::ReorgRecoversDependentTxs` (T4, T9, T10, T11, T12, T13) — A detached parent and its dependent child are both recovered through the node reorg callback and remain committable as one dependency-ordered pool graph. Paired units: `sort_txs_by_dependencies_orders_parents_before_children`, `reorg_direct_replay_treats_pool_duplicates_as_idempotent`. Command: `make integration CKB_TEST_ARGS='-c 1 ReorgRecoversDependentTxs'`
- `normal-mining-reorg-tree`: `test/src/specs/tx_pool/reorg_recovers_dependent.rs::ReorgRecoversDependentPendingTree` (T9, T10, T12, T13) — A detached dependent tree is recovered as real Pending and is re-proposed and committed by normal get_block_template mining without relying on optional uncles or a hand-authored proposal block. Paired units: `reorg_publishes_only_the_final_status_after_multiple_transitions`, `reorg_demotes_stale_gap_to_pending`, `pending_proposals_filter_conflicting_uncle_subtree`. Command: `make integration CKB_TEST_ARGS='-c 1 ReorgRecoversDependentPendingTree'`

#### `TP-PERSIST-001` — Coherent persistence recovery point

Minimum command: `cargo nextest run -p ckb-tx-pool --features internal -E 'test(/(save_pool_captures_complete_reorg_ownership|dependent_chain_survives_save_and_restart|dispatcher_channel_close|persisted_file_orders|persistence_v2_rejects_oversized|persistence_loader_accepts_legacy|uak_persistence_receipt|uak_dropped_persistence)/)'`

Rust evidence:

- `dependent_chain_survives_save_and_restart` (T4, T9, T10, T11, T12, T13)
- `dispatcher_channel_close_quiesces_workers_and_persists_pool` (T7, T9, T10, T12, T13)
- `persisted_file_orders_expanded_dep_group_parents` (T6, T9, T10, T12, T13)
- `persistence_loader_accepts_legacy_v1_vector` (T9, T10, T12, T13)
- `persistence_v2_rejects_oversized_file_before_reading_payload` (T3, T8, T9, T10, T12, T13)
- `save_pool_captures_complete_reorg_ownership` (T1, T2, T3, T9, T10, T12, T13)
- `uak_dropped_persistence_receipt_has_no_authority_effect` (T1, T2, T3, T4, T8, T9, T10, T11, T12, T13)
- `uak_persistence_receipt_is_coherent_and_parent_first` (T1, T2, T3, T4, T6, T8, T9, T10, T11, T12, T13)

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

Minimum command: `cargo nextest run -p ckb-tx-pool --features internal --lib -E 'test(/(reorg_demotes_stale_gap|pending_proposals_filter|full_reset_and_partial_priority_use_template_owned_tokens|full_rebuild_derives_uncles_from_candidate_authority|reorg_refresh_recovers_when_blank_reset_precedes_candidate_retention|full_rebuild_reissues_every_optimistic_delta|one_configured_script_owns_at_most_one_live_process_slot|selected_reader|conditional_cycle|uak_template|uak_accepted_timestamp|uak_chain_observation_reconciles|uak_runtime_chain_boundary_reconciles)/)'`

Rust evidence:

- `commit_and_removal_journal_block_assembler_delta` (T12, T13)
- `full_rebuild_derives_uncles_from_candidate_authority` (T3, T8, T9, T10, T11, T12, T13)
- `full_rebuild_reissues_every_optimistic_delta_generation` (T9, T10, T12, T13)
- `full_reset_and_partial_priority_use_template_owned_tokens` (T3, T8, T9, T10, T11, T12, T13)
- `one_configured_script_owns_at_most_one_live_process_slot` (T8, T10, T13)
- `pending_proposals_filter_conflicting_uncle_subtree` (T12, T13)
- `reorg_demotes_stale_gap_to_pending` (T12, T13)
- `reorg_refresh_recovers_when_blank_reset_precedes_candidate_retention` (T3, T8, T9, T10, T11, T12, T13)
- `uak_accepted_timestamp_is_part_of_the_immutable_source_cut` (T2, T9, T10, T12, T13)
- `uak_apply_advances_exact_template_source_versions` (T1, T2, T9, T10, T12, T13)
- `uak_chain_commit_updates_only_affected_template_package_scores` (T3, T4, T8, T9, T10, T12, T13)
- `uak_chain_observation_reconciles_every_gap_without_changed_proposal_hint` (T1, T2, T4, T9, T10, T12, T13)
- `uak_dropped_reset_build_remains_level_triggered_until_publication` (T3, T7, T9, T10, T12, T13)
- `uak_recovered_tree_has_normal_template_proposal_path` (T4, T9, T10, T11, T12, T13)
- `uak_requested_reset_fences_an_older_full_before_reset_publication` (T3, T9, T10, T12, T13)
- `uak_runtime_chain_boundary_reconciles_indexed_gap_against_paired_snapshot` (T1, T2, T4, T9, T10, T12, T13)
- `uak_template_counter_exhaustion_is_typed_and_mutation_free` (T3, T7, T8, T10, T11, T12, T13)
- `uak_template_cycle_fallback_preserves_descendant_aware_strength` (T3, T4, T8, T10, T11, T12, T13)
- `uak_template_dependency_budget_cannot_censor_later_independent_work` (T3, T4, T8, T10, T11, T12, T13)
- `uak_template_read_receipt_shares_order_and_complete_resolved_payload` (T1, T2, T3, T4, T9, T10, T12, T13)
- `uak_template_receipts_repair_overwrite_and_delayed_delta` (T9, T10, T12, T13)

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
- `uak_runtime_chain_boundary_commits_compact_hash_cache_with_snapshot` (T1, T2, T3, T5, T6, T9)
- `uak_runtime_chain_boundary_preserves_block_order_for_short_id_collisions` (T1, T2, T3, T5, T6, T9)
- `uak_trusted_witness_replacement_preserves_ingress_and_changes_payload_blame` (T1, T2, T3, T4, T5, T7, T8, T11)
- `verification_cache_isolated_by_witness_hash_not_raw_hash` (T1, T2, T3, T5, T6)

#### `TP-PERF-001` — Bounded attacker-controlled work

Minimum command: `cargo nextest run -p ckb-tx-pool --lib -E 'test(/(owner_fairness_and_active_caps|large_cycle_population|expiry_batch_is_bounded|randomized_public_transitions)/)'`

Rust evidence:

- `controller_dependent_secp_chain_reverse` (T1, T2, T3, T4, T7, T8, T10, T11, T13)
- `descendants_cache_members_stay_within_budget` (T3, T8, T10, T11, T13)
- `large_cycle_population_is_partitioned_from_small_head` (T1, T2, T3, T4, T7, T8, T10, T11, T13)
- `owner_fairness_and_active_caps_do_not_scan_a_capped_prefix` (T3, T8, T10, T11, T13)
- `successful_stage_completion_checks_out_same_lane_without_projection_drift` (T1, T2, T3, T4, T7, T8, T10, T11, T13)
- `uak_new_trusted_owner_joins_the_existing_owner_ring_without_starving_remote` (T1, T2, T3, T7, T8, T10, T11, T13)

Process-level evidence:

- `conditional-reader-fanout`: `test/src/specs/tx_pool/limit.rs::TxPoolLimitAncestorCount` (T3, T6, T8, T10, T11, T13) — Two thousand cell-dep readers coexist with a spender without becoming persistent ancestors, while a genuine causal chain still stops at the configured ancestor limit. Paired units: `popular_dep_readers_coexist_with_spender`, `dep_readers_do_not_count_as_spender_ancestors`, `causal_ancestor_limit_never_evicts_existing_entries`. Command: `make integration CKB_TEST_ARGS='-c 1 TxPoolLimitAncestorCount'`
- `same-lane-relay-continuation`: `test/src/specs/tx_pool/txs_relay_order.rs::TxsRelayOrder` (T1, T2, T3, T4, T7, T8, T10, T11, T13) — Ten dependent transactions submitted to one node traverse RelayV3 and the receiving node's pre-pool Resolve/Verify workers; after relay, every transaction remains observable in exactly one pending-or-orphan pool class, while unit evidence pins same-lane checkout, worker capability, and cancellation semantics. Paired units: `successful_stage_completion_checks_out_same_lane_without_projection_drift`, `verify_continuation_uses_the_checked_out_worker_capability`, `cancellation_finishes_one_checked_out_continuation_without_dropping_it`. Command: `make integration CKB_TEST_ARGS='-c 1 TxsRelayOrder'`

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
