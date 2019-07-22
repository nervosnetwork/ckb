use crate::block_status::BlockStatus;
use crate::relayer::compact_block::CompactBlock;
use crate::relayer::compact_block_verifier::CompactBlockVerifier;
use crate::relayer::error::{Error, Ignored, Internal, Misbehavior};
use crate::relayer::Relayer;
use ckb_core::header::Header;
use ckb_core::BlockNumber;
use ckb_logger::debug_target;
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_protocol::{CompactBlock as FbsCompactBlock, RelayMessage};
use ckb_shared::shared::Shared;
use ckb_store::ChainStore;
use ckb_traits::{BlockMedianTimeContext, ChainProvider};
use ckb_verification::{HeaderResolver, HeaderVerifier, Verifier};
use failure::Error as FailureError;
use flatbuffers::FlatBufferBuilder;
use fnv::FnvHashSet;
use numext_fixed_hash::H256;
use std::convert::TryInto;
use std::sync::Arc;

pub struct CompactBlockProcess<'a, CS> {
    message: &'a FbsCompactBlock<'a>,
    relayer: &'a Relayer<CS>,
    nc: Arc<dyn CKBProtocolContext>,
    peer: PeerIndex,
}

#[derive(Debug, Eq, PartialEq)]
pub enum Status {
    // Accept block
    AcceptBlock,
    // Send missing_indexes by get_block_transactions
    SendMissingIndexes,
    // Send get_headers
    UnknownParent,
    // Should never reach
    MissingIndexesEmpty,
}

impl<'a, CS: ChainStore + 'static> CompactBlockProcess<'a, CS> {
    pub fn new(
        message: &'a FbsCompactBlock,
        relayer: &'a Relayer<CS>,
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
        let compact_block: CompactBlock = (*self.message).try_into()?;
        let block_hash = compact_block.header.hash().to_owned();

        let status = self.relayer.shared().get_block_status(&block_hash);
        if status.contains(BlockStatus::BLOCK_STORED) {
            return Err(Error::Ignored(Ignored::AlreadyStored).into());
        } else if status.contains(BlockStatus::BLOCK_INVALID) {
            debug_target!(
                crate::LOG_TARGET_RELAY,
                "receive a compact block with invalid status, {:#x}, peer: {}",
                block_hash,
                self.peer
            );
            return Err(Error::Misbehavior(Misbehavior::BlockInvalid).into());
        }

        let parent = self
            .relayer
            .shared
            .get_header_view(compact_block.header.parent_hash());
        if parent.is_none() {
            debug_target!(
                crate::LOG_TARGET_RELAY,
                "UnknownParent: {:#x}, send_getheaders_to_peer({})",
                block_hash,
                self.peer
            );
            self.relayer.shared.send_getheaders_to_peer(
                self.nc.as_ref(),
                self.peer,
                self.relayer.shared.lock_chain_state().tip_header(),
            );
            return Ok(Status::UnknownParent);
        }

        let parent = parent.unwrap();
        {
            let tip_header = self.relayer.shared.tip_header();
            let tip_header_view = self
                .relayer
                .shared
                .get_header_view(tip_header.hash())
                .expect("Get tip header view failed");
            let current_total_difficulty =
                parent.total_difficulty() + compact_block.header.difficulty();

            if tip_header_view
                .is_better_than(&current_total_difficulty, compact_block.header.hash())
            {
                debug_target!(
                    crate::LOG_TARGET_RELAY,
                    "Received a compact block({:#x}), total difficulty {:#x} <= {:#x}, ignore it",
                    block_hash,
                    current_total_difficulty,
                    tip_header_view.total_difficulty(),
                );
                return Err(Error::Ignored(Ignored::NotBetter).into());
            }
        }

        if let Some(flight) = self
            .relayer
            .shared()
            .read_inflight_blocks()
            .inflight_state_by_block(&block_hash)
        {
            if flight.peers.contains(&self.peer) {
                debug_target!(
                    crate::LOG_TARGET_RELAY,
                    "discard already in-flight compact block {:x}",
                    block_hash,
                );
                return Err(Error::Ignored(Ignored::AlreadyInFlight).into());
            }
        }

        // The new arrived has greater difficulty than local best known chain
        let missing_indexes: Vec<usize>;
        {
            // Verify compact block
            let mut pending_compact_blocks = self.relayer.shared().pending_compact_blocks();
            if pending_compact_blocks
                .get(&block_hash)
                .map(|(_, peers_set)| peers_set.contains(&self.peer))
                .unwrap_or(false)
            {
                debug_target!(
                    crate::LOG_TARGET_RELAY,
                    "discard already pending compact block {:x}",
                    block_hash
                );
                return Err(Error::Ignored(Ignored::AlreadyPending).into());
            } else {
                let fn_get_pending_header = {
                    |block_hash| {
                        pending_compact_blocks
                            .get(&block_hash)
                            .map(|(compact_block, _)| compact_block.header.to_owned())
                    }
                };
                let resolver = self
                    .relayer
                    .shared
                    .new_header_resolver(&compact_block.header, parent.into_inner());
                let header_verifier = HeaderVerifier::new(
                    CompactBlockMedianTimeView {
                        fn_get_pending_header: Box::new(fn_get_pending_header),
                        shared: self.relayer.shared.shared(),
                    },
                    Arc::clone(&self.relayer.shared.consensus().pow_engine()),
                );
                let compact_block_verifier = CompactBlockVerifier::new();
                if let Err(err) = header_verifier.verify(&resolver) {
                    debug_target!(crate::LOG_TARGET_RELAY, "invalid header: {}", err);
                    self.relayer
                        .shared()
                        .insert_block_status(block_hash, BlockStatus::BLOCK_INVALID);
                    return Err(Error::Misbehavior(Misbehavior::HeaderInvalid).into());
                }
                compact_block_verifier.verify(&compact_block)?;

                // Header has been verified ok, update state
                let epoch = resolver.epoch().expect("epoch verified").clone();
                self.relayer
                    .shared()
                    .insert_valid_header(self.peer, &compact_block.header, epoch);
            }

            // Reconstruct block
            let ret = {
                self.relayer
                    .request_proposal_txs(self.nc.as_ref(), self.peer, &compact_block);
                self.relayer.reconstruct_block(&compact_block, Vec::new())
            };

            // Accept block
            // `relayer.accept_block` will make sure the validity of block before persisting
            // into database
            match ret {
                Ok(block) => {
                    pending_compact_blocks.remove(&block_hash);
                    self.relayer
                        .accept_block(self.nc.as_ref(), self.peer, block);
                    return Ok(Status::AcceptBlock);
                }
                Err(missing) => {
                    missing_indexes = missing;
                    pending_compact_blocks
                        .entry(block_hash.clone())
                        .or_insert_with(|| (compact_block, FnvHashSet::default()))
                        .1
                        .insert(self.peer);
                }
            }
        }

        // The missing_indexes should never be empty
        if missing_indexes.is_empty() {
            return Ok(Status::MissingIndexesEmpty);
        }

        if !self
            .relayer
            .shared()
            .write_inflight_blocks()
            .insert(self.peer, block_hash.to_owned())
        {
            debug_target!(
                crate::LOG_TARGET_RELAY,
                "BlockInFlight reach limit or had requested, peer: {}, block: {:x}",
                self.peer,
                block_hash,
            );
            return Err(Error::Internal(Internal::InflightBlocksReachLimit).into());
        }

        let fbb = &mut FlatBufferBuilder::new();
        let message = RelayMessage::build_get_block_transactions(
            fbb,
            &block_hash,
            &missing_indexes
                .into_iter()
                .map(|i| i as u32)
                .collect::<Vec<_>>(),
        );
        fbb.finish(message, None);
        if let Err(err) = self
            .nc
            .send_message_to(self.peer, fbb.finished_data().into())
        {
            ckb_logger::debug!("relayer send get_block_transactions error: {:?}", err);
        }
        Ok(Status::SendMissingIndexes)
    }
}

struct CompactBlockMedianTimeView<'a, CS> {
    fn_get_pending_header: Box<Fn(H256) -> Option<Header> + 'a>,
    shared: &'a Shared<CS>,
}

impl<'a, CS> CompactBlockMedianTimeView<'a, CS>
where
    CS: ChainStore,
{
    fn get_header(&self, hash: &H256) -> Option<Header> {
        (self.fn_get_pending_header)(hash.to_owned())
            .or_else(|| self.shared.store().get_block_header(hash))
    }
}

impl<'a, CS> BlockMedianTimeContext for CompactBlockMedianTimeView<'a, CS>
where
    CS: ChainStore,
{
    fn median_block_count(&self) -> u64 {
        self.shared.consensus().median_time_block_count() as u64
    }

    fn timestamp_and_parent(&self, block_hash: &H256) -> (u64, BlockNumber, H256) {
        let header = self
            .get_header(&block_hash)
            .expect("[CompactBlockMedianTimeView] blocks used for median time exist");
        (
            header.timestamp(),
            header.number(),
            header.parent_hash().to_owned(),
        )
    }
}
