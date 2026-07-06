use crate::component::orphan::Entry as OrphanEntry;
use crate::error::Reject;
use crate::service::TxVerificationResult;
use ckb_logger::warn;
use ckb_network::PeerIndex;
use ckb_types::core::{Cycle, TransactionView};
use ckb_types::packed::{Byte32, ProposalShortId};
use ckb_util::LinkedHashSet;
use std::collections::{HashSet, VecDeque};

impl super::TxPoolService {
    pub(crate) async fn orphan_contains(&self, tx: &TransactionView) -> bool {
        let orphan = self.orphan.read().await;
        orphan.contains_key(&tx.proposal_short_id())
    }

    /// Route a transaction with missing inputs to the orphan pool and notify
    /// the relayer about the missing parents.
    ///
    /// Used by both [`Self::after_process`] (which computes parents from the
    /// tx) and the ordered resolver (which receives parents from the resolve
    /// stage result).
    pub(crate) async fn handle_missing_input_orphan(
        &self,
        tx: TransactionView,
        peer: PeerIndex,
        declared_cycle: Cycle,
        parents: HashSet<Byte32>,
    ) {
        // Only notify the relayer after the tx has actually been accepted into
        // the orphan pool. This avoids telling peers about missing parents for
        // a tx that we end up dropping (e.g. duplicate orphan or pool full).
        if self.add_orphan(tx, peer, declared_cycle).await {
            self.send_result_to_relayer(TxVerificationResult::UnknownParents { peer, parents });
        }
    }

    pub(crate) async fn add_orphan(
        &self,
        tx: TransactionView,
        peer: PeerIndex,
        declared_cycle: Cycle,
    ) -> bool {
        let (added, evicted_txs) =
            self.orphan
                .write()
                .await
                .add_orphan_tx(tx, peer, declared_cycle);
        // for any evicted orphan tx, we should send reject to relayer
        // so that we mark it as `unknown` in filter
        for tx_hash in evicted_txs {
            self.send_result_to_relayer(TxVerificationResult::Reject { tx_hash });
        }
        added
    }

    pub(crate) async fn find_orphan_by_previous(&self, tx: &TransactionView) -> Vec<OrphanEntry> {
        let orphan = self.orphan.read().await;
        orphan
            .find_by_previous(tx)
            .iter()
            .filter_map(|id| orphan.get(id).cloned())
            .collect::<Vec<_>>()
    }

    pub(crate) async fn remove_orphan_tx(&self, id: &ProposalShortId) {
        self.orphan.write().await.remove_orphan_tx(id);
    }

    /// Remove all orphans which are resolved by the given transaction.
    ///
    /// The search is breadth-first: each orphan is routed through the same
    /// pipeline entry point as other remote transactions. When an orphan is
    /// eventually verified and submitted, `after_process` will recursively
    /// process its own descendants in the orphan pool.
    pub(crate) async fn process_orphan_tx(&self, tx: &TransactionView) {
        let mut orphan_queue: VecDeque<TransactionView> = VecDeque::new();
        orphan_queue.push_back(tx.clone());

        while let Some(previous) = orphan_queue.pop_front() {
            let orphans = self.find_orphan_by_previous(&previous).await;
            for orphan in orphans.into_iter() {
                let orphan_id = orphan.tx.proposal_short_id();

                #[cfg(feature = "pipeline")]
                {
                    match self
                        .classify_and_enqueue_tx(
                            orphan.tx.clone(),
                            false,
                            Some((orphan.cycle, orphan.peer)),
                        )
                        .await
                    {
                        Ok(_) => {
                            self.remove_orphan_tx(&orphan_id).await;
                            // The orphan is now in the pipeline. Its own children
                            // will be processed once it successfully submits via
                            // the normal `after_process` -> `handle_verify_success`
                            // path, so we do not need to push it back here.
                        }
                        Err(reject) => {
                            // Keep the orphan if the only problem is that its
                            // parents are not yet available or the pipeline queues
                            // are temporarily full.  For any other reject reason
                            // (malformed, low fee, etc.) remove it and notify the
                            // peer.
                            if crate::util::is_missing_input(&reject)
                                || matches!(reject, Reject::Full(_))
                            {
                                warn!(
                                    "process_orphan {} not ready ({reject}); keeping orphan from {}",
                                    orphan.tx.hash(),
                                    tx.hash(),
                                );
                            } else {
                                self.remove_orphan_tx(&orphan_id).await;
                                self.handle_remote_reject(&orphan.tx.hash(), &reject, orphan.peer)
                                    .await;
                            }
                        }
                    }
                }

                #[cfg(not(feature = "pipeline"))]
                {
                    if let Some((ret, snapshot)) = self
                        .process_tx_sync(orphan.tx.clone(), Some(orphan.cycle), None)
                        .await
                    {
                        let remote = Some((orphan.cycle, orphan.peer));
                        let keep = matches!(&ret, Err(reject) if crate::util::is_missing_input(reject) || matches!(reject, Reject::Full(_)));
                        if !keep {
                            self.remove_orphan_tx(&orphan_id).await;
                        }
                        // after_process handles remote reject notifications
                        // internally; do NOT call handle_remote_reject here to
                        // avoid double ban/relay/recent_reject.
                        self.after_process(orphan.tx, remote, &snapshot, &ret).await;
                    }
                }
            }
        }
    }

    pub(crate) async fn remove_orphan_txs_by_attach(&self, txs: &LinkedHashSet<TransactionView>) {
        // CRITICAL: this must run after `update_tx_pool_for_reorg` has replaced
        // `tx_pool.snapshot` with the post-attachment snapshot. Because the snapshot
        // already reflects the attached blocks, an orphan whose input was consumed by
        // one of those blocks resolves to `CellStatus::Dead` and is rejected here,
        // instead of being accepted back into the pipeline.
        for tx in txs.iter() {
            self.process_orphan_tx(tx).await;
        }
        let mut orphan = self.orphan.write().await;
        orphan.remove_orphan_txs(txs.iter().map(|tx| tx.proposal_short_id()));
    }
}
