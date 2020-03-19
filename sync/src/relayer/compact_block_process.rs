use crate::block_status::BlockStatus;
use crate::relayer::compact_block_verifier::CompactBlockVerifier;
use crate::relayer::{ReconstructionResult, Relayer};
use crate::{attempt, Status, StatusCode};
use ckb_chain_spec::consensus::Consensus;
use ckb_logger::{self, debug_target, metric};
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_store::{ChainDB, ChainStore};
use ckb_traits::BlockMedianTimeContext;
use ckb_types::{
    core::{self, BlockNumber},
    packed,
    prelude::*,
};
use ckb_verification::{HeaderVerifier, Verifier};
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

    pub fn execute(self) -> Status {
        let shared = self.relayer.shared();
        {
            let compact_block = self.message;
            if compact_block.uncles().len() > shared.consensus().max_uncles_num() {
                return StatusCode::ProtocolMessageIsMalformed.with_context(format!(
                    "CompactBlock uncles count({}) > consensus max_uncles_num({})",
                    compact_block.uncles().len(),
                    shared.consensus().max_uncles_num()
                ));
            }
            if (compact_block.proposals().len() as u64)
                > shared.consensus().max_block_proposals_limit()
            {
                return StatusCode::ProtocolMessageIsMalformed.with_context(format!(
                    "CompactBlock proposals count({}) > consensus max_block_proposals_limit({})",
                    compact_block.proposals().len(),
                    shared.consensus().max_block_proposals_limit(),
                ));
            }
        }

        let compact_block = self.message.to_entity();
        let header = compact_block.header().into_view();
        let block_hash = header.hash();

        // Only accept blocks with a height greater than tip - N
        // where N is the current epoch length

        let active_chain = shared.active_chain();
        let tip = active_chain.tip_header();
        let epoch_length = active_chain.epoch_ext().length();
        let lowest_number = tip.number().saturating_sub(epoch_length);

        if lowest_number > header.number() {
            return StatusCode::CompactBlockIsStaled.with_context(block_hash);
        }

        let status = active_chain.get_block_status(&block_hash);
        if status.contains(BlockStatus::BLOCK_STORED) {
            return StatusCode::CompactBlockAlreadyStored.with_context(block_hash);
        } else if status.contains(BlockStatus::BLOCK_INVALID) {
            return StatusCode::BlockIsInvalid.with_context(block_hash);
        }

        let parent = shared.get_header_view(&header.data().raw().parent_hash());
        if parent.is_none() {
            debug_target!(
                crate::LOG_TARGET_RELAY,
                "UnknownParent: {}, send_getheaders_to_peer({})",
                block_hash,
                self.peer
            );
            active_chain.send_getheaders_to_peer(self.nc.as_ref(), self.peer, &tip);
            return StatusCode::CompactBlockRequiresParent.with_context(format!(
                "{} parent: {}",
                block_hash,
                header.data().raw().parent_hash(),
            ));
        }

        let parent = parent.unwrap();

        if let Some(flight) = shared
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
                return StatusCode::CompactBlockIsAlreadyInFlight.with_context(block_hash);
            }
        }

        // The new arrived has greater difficulty than local best known chain
        let missing_transactions: Vec<u32>;
        let missing_uncles: Vec<u32>;
        let mut collision = false;
        {
            // Verify compact block
            let mut pending_compact_blocks = shared.state().pending_compact_blocks();
            if pending_compact_blocks
                .get(&block_hash)
                .map(|(_, peers_map)| peers_map.contains_key(&self.peer))
                .unwrap_or(false)
            {
                return StatusCode::CompactBlockIsAlreadyPending.with_context(block_hash);
            } else {
                let fn_get_pending_header = {
                    |block_hash| {
                        pending_compact_blocks
                            .get(&block_hash)
                            .map(|(compact_block, _)| compact_block.header().into_view())
                            .or_else(|| {
                                shared
                                    .get_header_view(&block_hash)
                                    .map(|header_view| header_view.into_inner())
                            })
                    }
                };
                let resolver = shared.new_header_resolver(&header, parent.into_inner());
                let median_time_context = CompactBlockMedianTimeView {
                    fn_get_pending_header: Box::new(fn_get_pending_header),
                    consensus: shared.consensus(),
                    store: shared.store(),
                };
                let header_verifier =
                    HeaderVerifier::new(&median_time_context, &shared.consensus());
                if let Err(err) = header_verifier.verify(&resolver) {
                    shared
                        .state()
                        .insert_block_status(block_hash.clone(), BlockStatus::BLOCK_INVALID);
                    return StatusCode::CompactBlockHasInvalidHeader
                        .with_context(format!("{} {}", block_hash, err));
                }
                attempt!(CompactBlockVerifier::verify(&compact_block));

                // Header has been verified ok, update state
                shared.insert_valid_header(self.peer, &header);
            }

            // Request proposal
            {
                let proposals: Vec<_> = compact_block.proposals().into_iter().collect();
                self.relayer.request_proposal_txs(
                    self.nc.as_ref(),
                    self.peer,
                    block_hash.clone(),
                    proposals,
                );
            }

            // Reconstruct block
            let ret =
                self.relayer
                    .reconstruct_block(&active_chain, &compact_block, vec![], &[], &[]);

            // Accept block
            // `relayer.accept_block` will make sure the validity of block before persisting
            // into database
            match ret {
                ReconstructionResult::Block(block) => {
                    pending_compact_blocks.remove(&block_hash);
                    self.relayer
                        .accept_block(self.nc.as_ref(), self.peer, block);
                    return Status::ok();
                }
                ReconstructionResult::Missing(transactions, uncles) => {
                    missing_transactions = transactions.into_iter().map(|i| i as u32).collect();
                    missing_uncles = uncles.into_iter().map(|i| i as u32).collect();
                }
                ReconstructionResult::Collided => {
                    missing_transactions = compact_block
                        .short_id_indexes()
                        .into_iter()
                        .map(|i| i as u32)
                        .collect();
                    collision = true;
                    missing_uncles = vec![];
                }
                ReconstructionResult::Error(status) => {
                    return status;
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
        if !shared
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
            return StatusCode::BlocksInFlightReachLimit.with_context(block_hash);
        }

        let status = if collision {
            StatusCode::CompactBlockMeetsShortIdsCollision.with_context(&block_hash)
        } else {
            StatusCode::CompactBlockRequiresFreshTransactions.with_context(&block_hash)
        };
        if !missing_transactions.is_empty() {
            metric!({
                "topic": "fresh_transactions",
                "tags": { "status": format!("{:?}", status.code()), },
                "fields": { "count": missing_transactions.len(), },
            });
        }
        if !missing_uncles.is_empty() {
            metric!({
                "topic": "fresh_uncles",
                "tags": { "status": format!("{:?}", status.code()), },
                "fields": { "count": missing_uncles.len(), },
            });
        }

        let content = packed::GetBlockTransactions::new_builder()
            .block_hash(block_hash)
            .indexes(missing_transactions.pack())
            .uncle_indexes(missing_uncles.pack())
            .build();
        let message = packed::RelayMessage::new_builder().set(content).build();
        let data = message.as_slice().into();
        if let Err(err) = self.nc.send_message_to(self.peer, data) {
            return StatusCode::Network
                .with_context(format!("Send GetBlockTransactions error: {:?}", err));
        }

        status
    }
}

struct CompactBlockMedianTimeView<'a> {
    fn_get_pending_header: Box<dyn Fn(packed::Byte32) -> Option<core::HeaderView> + 'a>,
    store: &'a ChainDB,
    consensus: &'a Consensus,
}

impl<'a> CompactBlockMedianTimeView<'a> {
    fn get_header(&self, hash: &packed::Byte32) -> Option<core::HeaderView> {
        (self.fn_get_pending_header)(hash.to_owned()).or_else(|| self.store.get_block_header(hash))
    }
}

impl<'a> BlockMedianTimeContext for CompactBlockMedianTimeView<'a> {
    fn median_block_count(&self) -> u64 {
        self.consensus.median_time_block_count() as u64
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
