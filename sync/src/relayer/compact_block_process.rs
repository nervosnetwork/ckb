use crate::block_status::BlockStatus;
use crate::relayer::compact_block_verifier::CompactBlockVerifier;
use crate::relayer::{ReconstructionResult, Relayer};
use crate::types::{ActiveChain, PendingCompactBlockMap};
use crate::utils::send_message_to;
use crate::SyncShared;
use crate::{attempt, Status, StatusCode};
use ckb_chain_spec::consensus::Consensus;
use ckb_logger::{self, debug_target};
use ckb_metrics::metrics;
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_systemtime::unix_time_as_millis;
use ckb_traits::HeaderProvider;
use ckb_types::core::HeaderView;
use ckb_types::packed::Byte32;
use ckb_types::packed::CompactBlock;
use ckb_types::{core, packed, prelude::*};
use ckb_util::shrink_to_fit;
use ckb_util::MutexGuard;
use ckb_verification::{HeaderError, HeaderVerifier};
use ckb_verification_traits::Verifier;
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
        let active_chain = shared.active_chain();
        let compact_block = self.message.to_entity();
        let header = compact_block.header().into_view();
        let block_hash = header.hash();

        let status =
            non_contextual_check(&compact_block, &header, shared.consensus(), &active_chain);
        if !status.is_ok() {
            return status;
        }

        let status = contextual_check(&header, shared, &active_chain, &self.nc, self.peer);
        if !status.is_ok() {
            return status;
        }

        // The new arrived has greater difficulty than local best known chain
        attempt!(CompactBlockVerifier::verify(&compact_block));
        // Header has been verified ok, update state
        shared.insert_valid_header(self.peer, &header);

        // Request proposal
        let proposals: Vec<_> = compact_block.proposals().into_iter().collect();
        self.relayer.request_proposal_txs(
            self.nc.as_ref(),
            self.peer,
            (header.number(), block_hash.clone()).into(),
            proposals,
        );

        let mut pending_compact_blocks = shared.state().pending_compact_blocks();

        // Reconstruct block
        let ret = self
            .relayer
            .reconstruct_block(&active_chain, &compact_block, vec![], &[], &[]);

        // Accept block
        // `relayer.accept_block` will make sure the validity of block before persisting
        // into database
        match ret {
            ReconstructionResult::Block(block) => {
                metrics!(
                    counter,
                    "ckb.relay.cb_transaction_count",
                    block.transactions().len() as u64
                );
                metrics!(counter, "ckb.relay.cb_reconstruct_ok", 1);

                pending_compact_blocks.remove(&block_hash);
                // remove all pending request below this block epoch
                //
                // use epoch as the judgment condition because we accept
                // all block in current epoch as uncle block
                pending_compact_blocks.retain(|_, (v, _, _)| {
                    Unpack::<core::EpochNumberWithFraction>::unpack(
                        &v.header().as_reader().raw().epoch(),
                    )
                    .number()
                        >= block.epoch().number()
                });
                shrink_to_fit!(pending_compact_blocks, 20);
                self.relayer
                    .accept_block(self.nc.as_ref(), self.peer, block);

                Status::ok()
            }
            ReconstructionResult::Missing(transactions, uncles) => {
                let missing_transactions: Vec<u32> =
                    transactions.into_iter().map(|i| i as u32).collect();
                metrics!(
                    counter,
                    "ckb.relay.cb_fresh_tx_cnt",
                    missing_transactions.len() as u64
                );
                metrics!(counter, "ckb.relay.cb_reconstruct_fail", 1);

                let missing_uncles: Vec<u32> = uncles.into_iter().map(|i| i as u32).collect();
                missing_or_collided_post_process(
                    compact_block,
                    block_hash.clone(),
                    pending_compact_blocks,
                    self.nc,
                    missing_transactions,
                    missing_uncles,
                    self.peer,
                );

                StatusCode::CompactBlockRequiresFreshTransactions.with_context(&block_hash)
            }
            ReconstructionResult::Collided => {
                let missing_transactions: Vec<u32> = compact_block
                    .short_id_indexes()
                    .into_iter()
                    .map(|i| i as u32)
                    .collect();
                let missing_uncles: Vec<u32> = vec![];
                missing_or_collided_post_process(
                    compact_block,
                    block_hash.clone(),
                    pending_compact_blocks,
                    self.nc,
                    missing_transactions,
                    missing_uncles,
                    self.peer,
                );
                StatusCode::CompactBlockMeetsShortIdsCollision.with_context(&block_hash)
            }
            ReconstructionResult::Error(status) => status,
        }
    }
}

struct CompactBlockMedianTimeView<'a> {
    fn_get_pending_header: Box<dyn Fn(packed::Byte32) -> Option<core::HeaderView> + 'a>,
}

impl<'a> HeaderProvider for CompactBlockMedianTimeView<'a> {
    fn get_header(&self, hash: &packed::Byte32) -> Option<core::HeaderView> {
        // Note: don't query store because we already did that in `fn_get_pending_header -> get_header_view`.
        (self.fn_get_pending_header)(hash.to_owned())
    }
}

/// * check compact block's uncles and proposals length
/// * check compact block height
fn non_contextual_check(
    compact_block: &CompactBlock,
    header: &HeaderView,
    consensus: &Consensus,
    active_chain: &ActiveChain,
) -> Status {
    if compact_block.uncles().len() > consensus.max_uncles_num() {
        return StatusCode::ProtocolMessageIsMalformed.with_context(format!(
            "CompactBlock uncles count({}) > consensus max_uncles_num({})",
            compact_block.uncles().len(),
            consensus.max_uncles_num()
        ));
    }
    if (compact_block.proposals().len() as u64) > consensus.max_block_proposals_limit() {
        return StatusCode::ProtocolMessageIsMalformed.with_context(format!(
            "CompactBlock proposals count({}) > consensus max_block_proposals_limit({})",
            compact_block.proposals().len(),
            consensus.max_block_proposals_limit(),
        ));
    }

    // Only accept blocks with a height greater than tip - N
    // where N is the current epoch length
    let block_hash = header.hash();
    let tip = active_chain.tip_header();
    let epoch_length = active_chain.epoch_ext().length();
    let lowest_number = tip.number().saturating_sub(epoch_length);

    if lowest_number > header.number() {
        return StatusCode::CompactBlockIsStaled.with_context(block_hash);
    }

    Status::ok()
}

/// * check compact block if already stored in db
/// * check compact block extension validation
/// * check compact block's parent block is not stored in db
/// * check compact block is in pending
/// * check compact header verification
fn contextual_check(
    compact_block_header: &HeaderView,
    shared: &Arc<SyncShared>,
    active_chain: &ActiveChain,
    nc: &Arc<dyn CKBProtocolContext>,
    peer: PeerIndex,
) -> Status {
    let block_hash = compact_block_header.hash();
    let tip = active_chain.tip_header();

    let status = active_chain.get_block_status(&block_hash);
    if status.contains(BlockStatus::BLOCK_STORED) {
        // update last common header and best known
        let parent = shared
            .get_header_view(&compact_block_header.data().raw().parent_hash(), Some(true))
            .expect("parent block must exist");
        let header_view = {
            let total_difficulty = parent.total_difficulty() + compact_block_header.difficulty();
            crate::types::HeaderView::new(compact_block_header.clone(), total_difficulty)
        };

        let state = shared.state().peers();
        state.may_set_best_known_header(peer, header_view);

        return StatusCode::CompactBlockAlreadyStored.with_context(block_hash);
    } else if status.contains(BlockStatus::BLOCK_RECEIVED) {
        // block already in orphan pool
        return Status::ignored();
    } else if status.contains(BlockStatus::BLOCK_INVALID) {
        return StatusCode::BlockIsInvalid.with_context(block_hash);
    }

    let store_first = tip.number() + 1 >= compact_block_header.number();
    let parent = shared.get_header_view(
        &compact_block_header.data().raw().parent_hash(),
        Some(store_first),
    );
    if parent.is_none() {
        debug_target!(
            crate::LOG_TARGET_RELAY,
            "UnknownParent: {}, send_getheaders_to_peer({})",
            block_hash,
            peer
        );
        active_chain.send_getheaders_to_peer(nc.as_ref(), peer, &tip);
        return StatusCode::CompactBlockRequiresParent.with_context(format!(
            "{} parent: {}",
            block_hash,
            compact_block_header.data().raw().parent_hash(),
        ));
    }

    // compact block is in pending
    let pending_compact_blocks = shared.state().pending_compact_blocks();
    if pending_compact_blocks
        .get(&block_hash)
        .map(|(_, peers_map, _)| peers_map.contains_key(&peer))
        .unwrap_or(false)
    {
        return StatusCode::CompactBlockIsAlreadyPending.with_context(block_hash);
    }

    // compact header verification
    let fn_get_pending_header = {
        |block_hash| {
            pending_compact_blocks
                .get(&block_hash)
                .map(|(compact_block, _, _)| compact_block.header().into_view())
                .or_else(|| {
                    shared
                        .get_header_view(&block_hash, None)
                        .map(|header_view| header_view.into_inner())
                })
        }
    };
    let median_time_context = CompactBlockMedianTimeView {
        fn_get_pending_header: Box::new(fn_get_pending_header),
    };
    let header_verifier = HeaderVerifier::new(&median_time_context, shared.consensus());
    if let Err(err) = header_verifier.verify(compact_block_header) {
        if err
            .downcast_ref::<HeaderError>()
            .map(|e| e.is_too_new())
            .unwrap_or(false)
        {
            return Status::ignored();
        } else {
            shared
                .state()
                .insert_block_status(block_hash.clone(), BlockStatus::BLOCK_INVALID);
            return StatusCode::CompactBlockHasInvalidHeader
                .with_context(format!("{block_hash} {err}"));
        }
    }

    Status::ok()
}

/// request missing txs and uncles from peer
fn missing_or_collided_post_process(
    compact_block: CompactBlock,
    block_hash: Byte32,
    mut pending_compact_blocks: MutexGuard<PendingCompactBlockMap>,
    nc: Arc<dyn CKBProtocolContext>,
    missing_transactions: Vec<u32>,
    missing_uncles: Vec<u32>,
    peer: PeerIndex,
) {
    pending_compact_blocks
        .entry(block_hash.clone())
        .or_insert_with(|| (compact_block, HashMap::default(), unix_time_as_millis()))
        .1
        .insert(peer, (missing_transactions.clone(), missing_uncles.clone()));

    let content = packed::GetBlockTransactions::new_builder()
        .block_hash(block_hash)
        .indexes(missing_transactions.pack())
        .uncle_indexes(missing_uncles.pack())
        .build();
    let message = packed::RelayMessage::new_builder().set(content).build();
    let sending = send_message_to(nc.as_ref(), peer, &message);
    if !sending.is_ok() {
        ckb_logger::warn_target!(
            crate::LOG_TARGET_RELAY,
            "ignore the sending message error, error: {}",
            sending
        );
    }
}
