use crate::orphan_block_pool::OrphanBlockPool;
use crate::{
    tell_synchronizer_to_punish_the_bad_peer, LonelyBlockWithCallback, UnverifiedBlock,
    VerifiedBlockStatus, VerifyResult,
};
use ckb_channel::{select, Receiver, SendError, Sender};
use ckb_error::{Error, InternalErrorKind};
use ckb_logger::internal::trace;
use ckb_logger::{debug, error, info};
use ckb_shared::block_status::BlockStatus;
use ckb_shared::types::VerifyFailedBlockInfo;
use ckb_shared::Shared;
use ckb_store::ChainStore;
use ckb_systemtime::unix_time_as_millis;
use ckb_types::core::{BlockExt, BlockView, HeaderView};
use ckb_types::U256;
use ckb_verification::InvalidParentError;
use std::sync::Arc;

pub(crate) struct ConsumeOrphan {
    shared: Shared,
    orphan_blocks_broker: Arc<OrphanBlockPool>,
    lonely_blocks_rx: Receiver<LonelyBlockWithCallback>,
    unverified_blocks_tx: Sender<UnverifiedBlock>,

    verify_failed_blocks_tx: tokio::sync::mpsc::UnboundedSender<VerifyFailedBlockInfo>,

    stop_rx: Receiver<()>,
}

impl ConsumeOrphan {
    pub(crate) fn new(
        shared: Shared,
        orphan_block_pool: Arc<OrphanBlockPool>,
        unverified_blocks_tx: Sender<UnverifiedBlock>,
        lonely_blocks_rx: Receiver<LonelyBlockWithCallback>,

        verify_failed_blocks_tx: tokio::sync::mpsc::UnboundedSender<VerifyFailedBlockInfo>,
        stop_rx: Receiver<()>,
    ) -> ConsumeOrphan {
        ConsumeOrphan {
            shared,
            orphan_blocks_broker: orphan_block_pool,
            lonely_blocks_rx,
            unverified_blocks_tx,
            verify_failed_blocks_tx,
            stop_rx,
        }
    }

    pub(crate) fn start(&self) {
        loop {
            select! {
                recv(self.stop_rx) -> _ => {
                    info!("unverified_queue_consumer got exit signal, exit now");
                    return;
                },
                recv(self.lonely_blocks_rx) -> msg => match msg {
                    Ok(lonely_block) => {
                        self.process_lonely_block(lonely_block);
                    },
                    Err(err) => {
                        error!("lonely_block_rx err: {}", err);
                        return
                    }
                },
            }
        }
    }

    fn process_lonely_block(&self, lonely_block: LonelyBlockWithCallback) {
        let parent_hash = lonely_block.block().parent_hash();
        let parent_status = self.shared.get_block_status(&parent_hash);
        if parent_status.contains(BlockStatus::BLOCK_PARTIAL_STORED) {
            let parent_header = self
                .shared
                .store()
                .get_block_header(&parent_hash)
                .expect("parent already store");

            let unverified_block: UnverifiedBlock =
                lonely_block.combine_parent_header(parent_header);
            self.send_unverified_block(unverified_block);
        } else {
            self.orphan_blocks_broker.insert(lonely_block);
        }
        self.search_orphan_pool()
    }

    fn search_orphan_pool(&self) {
        for leader_hash in self.orphan_blocks_broker.clone_leaders() {
            if !self
                .shared
                .contains_block_status(&leader_hash, BlockStatus::BLOCK_PARTIAL_STORED)
            {
                trace!("orphan leader: {} not partial stored", leader_hash);
                continue;
            }

            let descendants: Vec<LonelyBlockWithCallback> = self
                .orphan_blocks_broker
                .remove_blocks_by_parent(&leader_hash);
            if descendants.is_empty() {
                error!(
                    "leader {} does not have any descendants, this shouldn't happen",
                    leader_hash
                );
                continue;
            }
            let (first_descendants_number, last_descendants_number, descendants_len) = (
                descendants
                    .first()
                    .expect("descdant not empty")
                    .block()
                    .number(),
                descendants
                    .last()
                    .expect("descdant not empty")
                    .block()
                    .number(),
                descendants.len(),
            );
            let accept_error_occurred = self.accept_descendants(descendants);

            if !accept_error_occurred {
                debug!(
                    "accept {} blocks [{}->{}] success",
                    descendants_len, first_descendants_number, last_descendants_number
                )
            }
        }
    }

    fn send_unverified_block(&self, unverified_block: UnverifiedBlock) -> bool {
        match self.unverified_blocks_tx.send(unverified_block) {
            Ok(_) => true,
            Err(SendError(unverified_block)) => {
                error!("send unverified_block_tx failed, the receiver has been closed");
                let err: Error = InternalErrorKind::System
                    .other(format!(
                        "send unverified_block_tx failed, the receiver have been close"
                    ))
                    .into();

                tell_synchronizer_to_punish_the_bad_peer(
                    self.verify_failed_blocks_tx.clone(),
                    unverified_block.peer_id(),
                    unverified_block.block().hash(),
                    &err,
                );

                let verify_result: VerifyResult = Err(err);
                unverified_block.execute_callback(verify_result);
                false
            }
        }
    }

    fn accept_descendants(&self, descendants: Vec<LonelyBlockWithCallback>) -> bool {
        let mut accept_error_occurred = false;
        for descendant_block in descendants {
            match self.accept_descendant(descendant_block.block().to_owned()) {
                Ok(accepted_opt) => match accepted_opt {
                    Some((parent_header, total_difficulty)) => {
                        let unverified_block: UnverifiedBlock =
                            descendant_block.combine_parent_header(parent_header);
                        let block_number = unverified_block.block().number();
                        let block_hash = unverified_block.block().hash();

                        if !self.send_unverified_block(unverified_block) {
                            continue;
                        }

                        if total_difficulty.gt(self.shared.get_unverified_tip().total_difficulty())
                        {
                            self.shared.set_unverified_tip(ckb_shared::HeaderIndex::new(
                                block_number.clone(),
                                block_hash.clone(),
                                total_difficulty,
                            ));
                            debug!("set unverified_tip to {}-{}, while unverified_tip - verified_tip = {}",
                            block_number.clone(),
                            block_hash.clone(),
                            block_number.saturating_sub(self.shared.snapshot().tip_number()))
                        } else {
                            debug!("received a block {}-{} with lower or equal difficulty than unverified_tip {}-{}",
                                    block_number,
                                    block_hash,
                                    self.shared.get_unverified_tip().number(),
                                    self.shared.get_unverified_tip().hash(),
                                    );
                        }
                    }
                    None => {
                        info!(
                            "doesn't accept block {}, because it has been stored",
                            descendant_block.block().hash()
                        );
                        let verify_result: VerifyResult =
                            Ok(VerifiedBlockStatus::PreviouslySeenButNotVerified);
                        descendant_block.execute_callback(verify_result);
                    }
                },

                Err(err) => {
                    accept_error_occurred = true;

                    tell_synchronizer_to_punish_the_bad_peer(
                        self.verify_failed_blocks_tx.clone(),
                        descendant_block.peer_id(),
                        descendant_block.block().hash(),
                        &err,
                    );

                    error!(
                        "accept block {} failed: {}",
                        descendant_block.block().hash(),
                        err
                    );

                    descendant_block.execute_callback(Err(err));
                }
            }
        }
        accept_error_occurred
    }

    fn accept_descendant(&self, block: Arc<BlockView>) -> Result<(HeaderView, U256), Error> {
        let (block_number, block_hash) = (block.number(), block.hash());

        let parent_header = self
            .shared
            .store()
            .get_block_header(&block.data().header().raw().parent_hash())
            .expect("parent already store");

        if let Some(ext) = self.shared.store().get_block_ext(&block.hash()) {
            debug!("block {}-{} has stored BlockExt", block_number, block_hash);
            return Ok(Some((parent_header, ext.total_difficulty)));
        }

        trace!("begin accept block: {}-{}", block.number(), block.hash());

        let parent_ext = self
            .shared
            .store()
            .get_block_ext(&block.data().header().raw().parent_hash())
            .expect("parent already store");

        if parent_ext.verified == Some(false) {
            return Err(InvalidParentError {
                parent_hash: parent_header.hash(),
            }
            .into());
        }

        let cannon_total_difficulty =
            parent_ext.total_difficulty.to_owned() + block.header().difficulty();

        let db_txn = Arc::new(self.shared.store().begin_transaction());

        let txn_snapshot = db_txn.get_snapshot();
        let _snapshot_block_ext = db_txn.get_update_for_block_ext(&block.hash(), &txn_snapshot);

        db_txn.insert_block(block.as_ref())?;

        let next_block_epoch = self
            .shared
            .consensus()
            .next_epoch_ext(&parent_header, &db_txn.borrow_as_data_loader())
            .expect("epoch should be stored");
        let new_epoch = next_block_epoch.is_head();
        let epoch = next_block_epoch.epoch();

        db_txn.insert_block_epoch_index(
            &block.header().hash(),
            &epoch.last_block_hash_in_previous_epoch(),
        )?;
        if new_epoch {
            db_txn.insert_epoch_ext(&epoch.last_block_hash_in_previous_epoch(), &epoch)?;
        }

        let ext = BlockExt {
            received_at: unix_time_as_millis(),
            total_difficulty: cannon_total_difficulty.clone(),
            total_uncles_count: parent_ext.total_uncles_count + block.data().uncles().len() as u64,
            verified: None,
            txs_fees: vec![],
            cycles: None,
            txs_sizes: None,
        };

        db_txn.insert_block_ext(&block.header().hash(), &ext)?;

        db_txn.commit()?;

        self.shared
            .insert_block_status(block_hash, BlockStatus::BLOCK_PARTIAL_STORED);

        Ok(Some((parent_header, cannon_total_difficulty)))
    }
}
