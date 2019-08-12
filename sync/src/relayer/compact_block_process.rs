use crate::block_status::BlockStatus;
use crate::relayer::compact_block::CompactBlock;
use crate::relayer::compact_block_verifier::CompactBlockVerifier;
use crate::relayer::Relayer;
use crate::{attempt, Status, StatusCode};
use ckb_core::header::Header;
use ckb_core::BlockNumber;
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_protocol::{CompactBlock as FbsCompactBlock, RelayMessage};
use ckb_shared::shared::Shared;
use ckb_store::ChainStore;
use ckb_traits::{BlockMedianTimeContext, ChainProvider};
use ckb_verification::{HeaderResolver, HeaderVerifier, Verifier};
use flatbuffers::FlatBufferBuilder;
use fnv::FnvHashMap;
use numext_fixed_hash::H256;
use std::convert::TryInto;
use std::sync::Arc;

pub struct CompactBlockProcess<'a> {
    message: &'a FbsCompactBlock<'a>,
    relayer: &'a Relayer,
    nc: Arc<dyn CKBProtocolContext>,
    peer: PeerIndex,
}

impl<'a> CompactBlockProcess<'a> {
    pub fn new(
        message: &'a FbsCompactBlock,
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
        let compact_block: CompactBlock =
            attempt!(TryInto::<CompactBlock>::try_into(*self.message));
        let block_hash = compact_block.header.hash().to_owned();
        let block_number = compact_block.header.number();

        // Only accept blocks with a height greater than tip - N
        // where N is the current epoch length
        let (lowest_number, tip) = {
            let cs = self.relayer.shared.lock_chain_state();
            let epoch_length = cs.current_epoch_ext().length();
            let tip = cs.tip_header().clone();

            (tip.number().saturating_sub(epoch_length), tip)
        };

        if lowest_number > compact_block.header.number() {
            return StatusCode::TooOldBlock.into();
        }

        let status = self.relayer.shared().get_block_status(&block_hash);
        if status.contains(BlockStatus::BLOCK_STORED) {
            return StatusCode::AlreadyStoredBlock.into();
        } else if status.contains(BlockStatus::BLOCK_INVALID) {
            return StatusCode::InvalidBlock.with_context(format!(
                "relay a mark-invalid CompactBlock from peer {}, #{} {:#x}",
                self.peer, block_number, block_hash
            ));
        }

        let parent = self
            .relayer
            .shared
            .get_header_view(compact_block.header.parent_hash());
        if parent.is_none() {
            self.relayer
                .shared
                .send_getheaders_to_peer(self.nc.as_ref(), self.peer, &tip);
            return StatusCode::WaitingParent.with_context(format!(
                "relay a missing-parent CompactBlock from peer {}, #{} {:#x}",
                self.peer, block_number, block_hash,
            ));
        }

        let parent = parent.unwrap();

        if let Some(flight) = self
            .relayer
            .shared()
            .read_inflight_blocks()
            .inflight_state_by_block(&block_hash)
        {
            if flight.peers.contains(&self.peer) {
                return StatusCode::AlreadyInFlightBlock.with_context(format!(
                    "relay an already-in-flight CompactBlock #{} {:#x}",
                    block_number, block_hash,
                ));
            }
        }

        // The new arrived has greater difficulty than local best known chain
        let missing_indexes: Vec<u32>;
        {
            // Verify compact block
            let mut pending_compact_blocks = self.relayer.shared().pending_compact_blocks();
            if pending_compact_blocks
                .get(&block_hash)
                .map(|(_, peers_map)| peers_map.contains_key(&self.peer))
                .unwrap_or(false)
            {
                return StatusCode::AlreadyPendingBlock.with_context(format!(
                    "relay an already-pending CompactBlock #{} {:#x}",
                    block_number, block_hash,
                ));
            } else {
                let fn_get_pending_header = {
                    |block_hash| {
                        pending_compact_blocks
                            .get(&block_hash)
                            .map(|(compact_block, _)| compact_block.header.to_owned())
                            .or_else(|| {
                                self.relayer
                                    .shared
                                    .get_header_view(&block_hash)
                                    .map(|header_view| header_view.into_inner())
                            })
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
                if let Err(err) = header_verifier.verify(&resolver) {
                    self.relayer
                        .shared()
                        .insert_block_status(block_hash.clone(), BlockStatus::BLOCK_INVALID);
                    return StatusCode::InvalidHeader.with_context(format!(
                        "relay a invalid-header CompactBlock #{} {:#x}, err: {}",
                        block_number, block_hash, err,
                    ));
                }
                attempt!(CompactBlockVerifier::verify(&compact_block));

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
                    return StatusCode::OK.into();
                }
                Err(missing) => {
                    missing_indexes = missing.into_iter().map(|i| i as u32).collect::<Vec<_>>();

                    assert!(!missing_indexes.is_empty());

                    pending_compact_blocks
                        .entry(block_hash.clone())
                        .or_insert_with(|| (compact_block, FnvHashMap::default()))
                        .1
                        .insert(self.peer, missing_indexes.clone());
                }
            }
        }

        if !self
            .relayer
            .shared()
            .write_inflight_blocks()
            .insert(self.peer, block_hash.to_owned())
        {
            return StatusCode::TooManyInFlightBlocks.with_context(format!(
                "relay reach BlockInFlight limit or had requested #{} {:#x}",
                block_number, block_hash,
            ));
        }

        let fbb = &mut FlatBufferBuilder::new();
        let message =
            RelayMessage::build_get_block_transactions(fbb, &block_hash, &missing_indexes);
        fbb.finish(message, None);
        if let Err(err) = self
            .nc
            .send_message_to(self.peer, fbb.finished_data().into())
        {
            ckb_logger::debug!("relayer send get_block_transactions error: {:?}", err);
        }

        StatusCode::WaitingTransactions.with_context(format!(
            "relay a missing-transactions({}) CompactBlock #{} {:#x}",
            block_number,
            missing_indexes.len(),
            block_hash,
        ))
    }
}

struct CompactBlockMedianTimeView<'a> {
    fn_get_pending_header: Box<Fn(H256) -> Option<Header> + 'a>,
    shared: &'a Shared,
}

impl<'a> CompactBlockMedianTimeView<'a> {
    fn get_header(&self, hash: &H256) -> Option<Header> {
        (self.fn_get_pending_header)(hash.to_owned())
            .or_else(|| self.shared.store().get_block_header(hash))
    }
}

impl<'a> BlockMedianTimeContext for CompactBlockMedianTimeView<'a> {
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
