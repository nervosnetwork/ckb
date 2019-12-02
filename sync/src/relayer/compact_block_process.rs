use crate::block_status::BlockStatus;
use crate::relayer::compact_block_verifier::CompactBlockVerifier;
use crate::relayer::error::{Error, Ignored, Internal, Misbehavior};
use crate::relayer::{ReconstructionError, Relayer};
use ckb_logger::{self, debug_target, warn};
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_shared::Snapshot;
use ckb_store::ChainStore;
use ckb_traits::BlockMedianTimeContext;
use ckb_types::{
    core::{self, BlockNumber, BuildHeaderContext, HeaderContext},
    packed,
    prelude::*,
};
use ckb_verification::{HeaderVerifier, Verifier};
use failure::{err_msg, Error as FailureError};
use std::collections::HashMap;
use std::sync::Arc;

// Keeping in mind that short_ids are expected to occasionally collide.
// On receiving compact-block message,
// while the reconstructed the block has a different transactions_root,
// 1. if all the transactions are prefilled,
// the node should ban the peer but not mark the block invalid
// because of the block hash may be wrong.
// 2. otherwise, there may be short_id collision in transaction pool,
// the node retreat to request all the short_ids from the peer.
pub struct CompactBlockProcess<'a> {
    message: packed::CompactBlockReader<'a>,
    relayer: &'a Relayer,
    nc: Arc<dyn CKBProtocolContext>,
    peer: PeerIndex,
}

#[derive(Debug, Eq, PartialEq)]
pub enum Status {
    // Accept block
    AcceptBlock,
    // Send get_headers
    UnknownParent,
    // Send missing_indexes by get_block_transactions
    SendMissingIndexes,
    // Collision and Send missing_indexes by get_block_transactions
    CollisionAndSendMissingIndexes,
}

impl<'a> CompactBlockProcess<'a> {
    pub fn new(
        message: packed::CompactBlockReader<'a>,
        relayer: &'a Relayer,
        nc: Arc<dyn CKBProtocolContext>,
        peer: PeerIndex,
    ) -> Self {
        CompactBlockProcess {
            message,
            nc,
            relayer,
            peer,
        }
    }

    pub fn execute(self) -> Result<Status, FailureError> {
        let snapshot = self.relayer.shared.snapshot();
        {
            let compact_block = self.message;
            if compact_block.uncles().len() > snapshot.consensus().max_uncles_num() {
                warn!("Peer {} sends us an invalid message, CompactBlock uncles size ({}) is greater than consensus max_uncles_num ({})",
                    self.peer, compact_block.uncles().len(), snapshot.consensus().max_uncles_num());
                return Err(err_msg(
                    "CompactBlock uncles size is greater than consensus max_uncles_num".to_owned(),
                ));
            }
            if (compact_block.proposals().len() as u64)
                > snapshot.consensus().max_block_proposals_limit()
            {
                warn!("Peer {} sends us an invalid message, CompactBlock proposals size ({}) is greater than consensus max_block_proposals_limit ({})",
                    self.peer, compact_block.proposals().len(), snapshot.consensus().max_block_proposals_limit());
                return Err(err_msg(
                    "CompactBlock proposals size is greater than consensus max_block_proposals_limit"
                        .to_owned(),
                ));
            }
        }

        let compact_block = self.message.to_entity();
        let header = compact_block.header().into_view();
        let block_hash = header.hash();

        // Only accept blocks with a height greater than tip - N
        // where N is the current epoch length

        let tip = snapshot.tip_header().clone();
        let epoch_length = snapshot.epoch_ext().length();
        let lowest_number = tip.number().saturating_sub(epoch_length);

        if lowest_number > header.number() {
            return Err(Error::Ignored(Ignored::TooOldBlock).into());
        }

        let status = snapshot.get_block_status(&block_hash);
        if status.contains(BlockStatus::BLOCK_STORED) {
            return Err(Error::Ignored(Ignored::AlreadyStored).into());
        } else if status.contains(BlockStatus::BLOCK_INVALID) {
            debug_target!(
                crate::LOG_TARGET_RELAY,
                "receive a compact block with invalid status, {}, peer: {}",
                block_hash,
                self.peer
            );
            return Err(Error::Misbehavior(Misbehavior::BlockInvalid).into());
        }

        let parent = snapshot.get_header_view(&header.data().raw().parent_hash());
        if parent.is_none() {
            debug_target!(
                crate::LOG_TARGET_RELAY,
                "UnknownParent: {}, send_getheaders_to_peer({})",
                block_hash,
                self.peer
            );
            snapshot.send_getheaders_to_peer(self.nc.as_ref(), self.peer, &tip);
            return Ok(Status::UnknownParent);
        }

        let parent = parent.unwrap();

        if let Some(flight) = snapshot
            .state()
            .read_inflight_blocks()
            .inflight_state_by_block(&block_hash)
        {
            if flight.peers.contains(&self.peer) {
                debug_target!(
                    crate::LOG_TARGET_RELAY,
                    "discard already in-flight compact block {}",
                    block_hash,
                );
                return Err(Error::Ignored(Ignored::AlreadyInFlight).into());
            }
        }

        // The new arrived has greater difficulty than local best known chain
        let missing_transactions: Vec<u32>;
        let missing_uncles: Vec<u32>;
        let mut collision = false;
        {
            // Verify compact block
            let mut pending_compact_blocks = snapshot.state().pending_compact_blocks();
            if pending_compact_blocks
                .get(&block_hash)
                .map(|(_, peers_map)| peers_map.contains_key(&self.peer))
                .unwrap_or(false)
            {
                debug_target!(
                    crate::LOG_TARGET_RELAY,
                    "discard already pending compact block {}",
                    block_hash
                );
                return Err(Error::Ignored(Ignored::AlreadyPending).into());
            } else {
                let fn_get_pending_header = {
                    |block_hash| {
                        pending_compact_blocks
                            .get(&block_hash)
                            .map(|(compact_block, _)| compact_block.header().into_view())
                            .or_else(|| {
                                snapshot
                                    .get_header_view(&block_hash)
                                    .map(|header_view| header_view.into_inner())
                            })
                    }
                };
                let header_ctx = compact_block
                    .build_header_context(self.relayer.shared.consensus().header_context_type());
                let resolver = snapshot.new_header_resolver(&header_ctx, parent.into_inner());
                let median_time_context = CompactBlockMedianTimeView {
                    fn_get_pending_header: Box::new(fn_get_pending_header),
                    snapshot: snapshot.store(),
                };
                let header_verifier =
                    HeaderVerifier::new(&median_time_context, &snapshot.consensus());
                if let Err(err) = header_verifier.verify(&resolver) {
                    debug_target!(crate::LOG_TARGET_RELAY, "invalid header: {}", err);
                    snapshot
                        .state()
                        .insert_block_status(block_hash, BlockStatus::BLOCK_INVALID);
                    return Err(Error::Misbehavior(Misbehavior::HeaderInvalid).into());
                }
                CompactBlockVerifier::verify(&compact_block)?;

                // Header has been verified ok, update state
                snapshot.insert_valid_header(self.peer, &header);
            }

            // Request proposal
            let proposals: Vec<_> = compact_block.proposals().into_iter().collect();
            if let Err(err) = self.relayer.request_proposal_txs(
                self.nc.as_ref(),
                self.peer,
                block_hash.clone(),
                proposals,
            ) {
                debug_target!(
                    crate::LOG_TARGET_RELAY,
                    "[CompactBlockProcess] request_proposal_txs: {}",
                    err
                );
            }

            // Reconstruct block
            let ret = self
                .relayer
                .reconstruct_block(&snapshot, &compact_block, vec![], &[], &[]);

            // Accept block
            // `relayer.accept_block` will make sure the validity of block before persisting
            // into database
            match ret {
                Ok(block) => {
                    pending_compact_blocks.remove(&block_hash);
                    self.relayer
                        .accept_block(&snapshot, self.nc.as_ref(), self.peer, block);
                    return Ok(Status::AcceptBlock);
                }
                Err(ReconstructionError::InvalidTransactionRoot) => {
                    return Err(Error::Misbehavior(Misbehavior::InvalidTransactionRoot).into());
                }
                Err(ReconstructionError::InvalidUncle) => {
                    return Err(Error::Misbehavior(Misbehavior::InvalidUncle).into());
                }
                Err(ReconstructionError::MissingIndexes(transactions, uncles)) => {
                    missing_transactions = transactions.into_iter().map(|i| i as u32).collect();
                    missing_uncles = uncles.into_iter().map(|i| i as u32).collect();
                }
                Err(ReconstructionError::Collision) => {
                    missing_transactions = compact_block
                        .short_id_indexes()
                        .into_iter()
                        .map(|i| i as u32)
                        .collect();
                    collision = true;
                    missing_uncles = vec![];
                }
                Err(ReconstructionError::Internal(e)) => {
                    ckb_logger::error!("reconstruct_block internal error: {}", e);
                    return Err(Error::Internal(Internal::TxPoolInternalError).into());
                }
            }

            pending_compact_blocks
                .entry(block_hash.clone())
                .or_insert_with(|| (compact_block, HashMap::default()))
                .1
                .insert(
                    self.peer,
                    (missing_transactions.clone(), missing_uncles.clone()),
                );
        }

        if !snapshot
            .state()
            .write_inflight_blocks()
            .insert(self.peer, block_hash.clone())
        {
            debug_target!(
                crate::LOG_TARGET_RELAY,
                "BlockInFlight reach limit or had requested, peer: {}, block: {}",
                self.peer,
                block_hash,
            );
            return Err(Error::Internal(Internal::InflightBlocksReachLimit).into());
        }

        let content = packed::GetBlockTransactions::new_builder()
            .block_hash(block_hash)
            .indexes(missing_transactions.pack())
            .uncle_indexes(missing_uncles.pack())
            .build();
        let message = packed::RelayMessage::new_builder().set(content).build();
        let data = message.as_slice().into();
        if let Err(err) = self.nc.send_message_to(self.peer, data) {
            ckb_logger::debug!("relayer send get_block_transactions error: {:?}", err);
        }

        if collision {
            Ok(Status::CollisionAndSendMissingIndexes)
        } else {
            Ok(Status::SendMissingIndexes)
        }
    }
}

struct CompactBlockMedianTimeView<'a> {
    fn_get_pending_header: Box<dyn Fn(packed::Byte32) -> Option<core::HeaderView> + 'a>,
    snapshot: &'a Snapshot,
}

impl<'a> CompactBlockMedianTimeView<'a> {
    fn get_header(&self, hash: &packed::Byte32) -> Option<core::HeaderView> {
        (self.fn_get_pending_header)(hash.to_owned())
            .or_else(|| self.snapshot.get_block_header(hash))
    }
}

impl<'a> BlockMedianTimeContext for CompactBlockMedianTimeView<'a> {
    fn median_block_count(&self) -> u64 {
        self.snapshot.consensus().median_time_block_count() as u64
    }

    fn timestamp_and_parent(
        &self,
        block_hash: &packed::Byte32,
    ) -> (u64, BlockNumber, packed::Byte32) {
        let header = self
            .get_header(&block_hash)
            .expect("[CompactBlockMedianTimeView] blocks used for median time exist");
        (
            header.timestamp(),
            header.number(),
            header.data().raw().parent_hash(),
        )
    }
}
