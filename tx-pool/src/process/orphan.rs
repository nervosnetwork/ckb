use crate::error::Reject;
use crate::service::TxVerificationResult;
use crate::tx_source::TxSource;
use ckb_logger::warn;
use ckb_types::core::TransactionView;
use ckb_types::packed::Byte32;
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
        source: TxSource,
        parents: HashSet<Byte32>,
    ) {
        // Only notify the relayer after the tx has actually been accepted into
        // the orphan pool. This avoids telling peers about missing parents for
        // a tx that we end up dropping (e.g. duplicate orphan or pool full).
        if self.add_orphan(tx, source).await
            && let Some(peer) = source.peer()
        {
            self.send_result_to_relayer(TxVerificationResult::UnknownParents { peer, parents });
        }
    }

    pub(crate) async fn add_orphan(&self, tx: TransactionView, source: TxSource) -> bool {
        let (added, evicted_txs) = self.orphan.write().await.add_orphan_tx(tx, source);
        // for any evicted orphan tx, we should send reject to relayer
        // so that we mark it as `unknown` in filter
        for tx_hash in evicted_txs {
            self.send_result_to_relayer(TxVerificationResult::Reject { tx_hash });
        }
        added
    }

    /// Remove all orphans which are resolved by the given transaction.
    ///
    /// The search is breadth-first: each orphan is routed through the same
    /// pipeline entry point as other remote transactions and block proposal
    /// notifications. When an orphan is eventually verified and submitted,
    /// `after_process` will recursively process its own descendants in the
    /// orphan pool.
    ///
    /// Removals are batched into a single write lock: an orphan's success or
    /// failure in the pipeline does not depend on its siblings being removed
    /// first, so there is no need to pay the cost of a write lock per orphan.
    pub(crate) async fn process_orphan_tx(&self, tx: &TransactionView) {
        let mut orphan_queue: VecDeque<TransactionView> = VecDeque::new();
        orphan_queue.push_back(tx.clone());

        while let Some(previous) = orphan_queue.pop_front() {
            // Collect the orphan entries under a single read lock, then process
            // them outside the lock. This keeps the critical section short and
            // avoids cloning transactions while holding the write lock.
            let orphans: Vec<_> = {
                let orphan = self.orphan.read().await;
                orphan
                    .find_by_previous(&previous)
                    .into_iter()
                    .cloned()
                    .filter_map(|id| orphan.get(&id).cloned().map(|entry| (id, entry)))
                    .collect()
            };

            let mut to_remove = Vec::new();
            for (orphan_id, orphan) in orphans.into_iter() {
                let orphan_hash = orphan.tx.hash();
                let orphan_peer = orphan.source.peer();

                match self.classify_and_enqueue_tx(orphan.tx, orphan.source).await {
                    Ok(_) => {
                        to_remove.push(orphan_id);
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
                                orphan_hash,
                                tx.hash(),
                            );
                        } else {
                            to_remove.push(orphan_id);
                            if let Some(peer) = orphan_peer {
                                self.handle_remote_reject(&orphan_hash, &reject, peer).await;
                            }
                        }
                    }
                }
            }

            if !to_remove.is_empty() {
                let mut orphan = self.orphan.write().await;
                for id in to_remove {
                    orphan.remove_orphan_tx(&id);
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
