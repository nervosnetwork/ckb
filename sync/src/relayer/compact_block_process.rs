use crate::relayer::compact_block::CompactBlock;
use crate::relayer::compact_block_verifier::CompactBlockVerifier;
use crate::relayer::Relayer;
use ckb_core::{header::Header, BlockNumber};
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_protocol::{CompactBlock as FbsCompactBlock, RelayMessage};
use ckb_shared::shared::Shared;
use ckb_store::ChainStore;
use ckb_traits::{BlockMedianTimeContext, ChainProvider};
use ckb_verification::{HeaderResolverWrapper, HeaderVerifier, Verifier};
use failure::Error as FailureError;
use flatbuffers::FlatBufferBuilder;
use fnv::FnvHashMap;
use log::debug;
use numext_fixed_hash::H256;
use std::convert::TryInto;
use std::sync::Arc;

pub struct CompactBlockProcess<'a, CS> {
    message: &'a FbsCompactBlock<'a>,
    relayer: &'a Relayer<CS>,
    nc: &'a CKBProtocolContext,
    peer: PeerIndex,
}

impl<'a, CS: ChainStore> CompactBlockProcess<'a, CS> {
    pub fn new(
        message: &'a FbsCompactBlock,
        relayer: &'a Relayer<CS>,
        nc: &'a CKBProtocolContext,
        peer: PeerIndex,
    ) -> Self {
        CompactBlockProcess {
            message,
            nc,
            relayer,
            peer,
        }
    }

    pub fn execute(self) -> Result<(), FailureError> {
        let compact_block: CompactBlock = (*self.message).try_into()?;
        let block_hash = compact_block.header.hash().to_owned();
        if let Some(parent_header_view) = self
            .relayer
            .shared
            .get_header_view(&compact_block.header.parent_hash())
        {
            let best_known_header = self.relayer.shared.best_known_header();
            let current_total_difficulty =
                parent_header_view.total_difficulty() + compact_block.header.difficulty();
            if current_total_difficulty <= *best_known_header.total_difficulty() {
                debug!(
                    target: "relay",
                    "Received a compact block({}), total difficulty {} <= {}, ignore it",
                    block_hash,
                    current_total_difficulty,
                    best_known_header.total_difficulty(),
                );
                return Ok(());
            }
        } else if self.relayer.shared.is_initial_block_download() {
            // If self is in the IBD state, do nothing
            return Ok(());
        } else {
            debug!(target: "relay", "UnknownParent: {}, send_getheaders_to_peer({})", block_hash, self.peer);
            self.relayer.shared.send_getheaders_to_peer(
                self.nc,
                self.peer,
                self.relayer.shared.chain_state().lock().tip_header(),
            );
            return Ok(());
        }

        // The new arrived has greater difficulty than local best known chain
        let mut missing_indexes: Vec<usize> = Vec::new();
        {
            // Verify compact block
            let mut pending_compact_blocks = self.relayer.state.pending_compact_blocks.lock();
            if pending_compact_blocks.get(&block_hash).is_some()
                || self.relayer.shared.get_block(&block_hash).is_some()
            {
                debug!(target: "relay", "already processed compact block {}", block_hash);
            } else {
                let resolver = HeaderResolverWrapper::new(
                    &compact_block.header,
                    self.relayer.shared.shared().to_owned(),
                );
                let header_verifier = HeaderVerifier::new(
                    CompactBlockMedianTimeView {
                        header: &compact_block.header,
                        pending_compact_blocks: &pending_compact_blocks,
                        shared: self.relayer.shared.shared(),
                    },
                    Arc::clone(&self.relayer.shared.consensus().pow_engine()),
                );
                let compact_block_verifier = CompactBlockVerifier::new();
                if let Err(err) = header_verifier.verify(&resolver) {
                    debug!(target: "relay", "unexpected header verify failed: {}", err);
                    return Ok(());
                }
                compact_block_verifier.verify(&compact_block)?;
            }

            // Reconstruct block
            let ret = {
                let chain_state = self.relayer.shared.chain_state().lock();
                self.relayer
                    .request_proposal_txs(&chain_state, self.nc, self.peer, &compact_block);
                self.relayer
                    .reconstruct_block(&chain_state, &compact_block, Vec::new())
            };

            // Accept block
            // `relayer.accept_block` will make sure the validity of block before persisting
            // into database
            match ret {
                Ok(block) => self
                    .relayer
                    .accept_block(self.nc, self.peer, &Arc::new(block)),
                Err(missing) => {
                    missing_indexes = missing;
                    pending_compact_blocks.insert(block_hash.clone(), compact_block);
                }
            }
        }
        if !missing_indexes.is_empty() {
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
            self.nc
                .send_message_to(self.peer, fbb.finished_data().into());
        }
        Ok(())
    }
}

struct CompactBlockMedianTimeView<'a, CS> {
    header: &'a Header,
    pending_compact_blocks: &'a FnvHashMap<H256, CompactBlock>,
    shared: &'a Shared<CS>,
}

impl<'a, CS> CompactBlockMedianTimeView<'a, CS>
where
    CS: ChainStore,
{
    fn get_header(&self, hash: &H256) -> Option<Header> {
        self.pending_compact_blocks
            .get(hash)
            .map(|cb| cb.header.to_owned())
            .or_else(|| self.shared.block_header(hash))
    }
}

impl<'a, CS> BlockMedianTimeContext for CompactBlockMedianTimeView<'a, CS>
where
    CS: ChainStore,
{
    fn median_block_count(&self) -> u64 {
        self.shared.consensus().median_time_block_count() as u64
    }

    fn timestamp(&self, _n: BlockNumber) -> Option<u64> {
        None
    }

    fn ancestor_timestamps(&self, block_number: BlockNumber) -> Vec<u64> {
        if Some(block_number) != self.header.number().checked_sub(1) {
            return Vec::new();
        }
        let count = std::cmp::min(self.median_block_count(), block_number + 1);
        let mut block_hash = self.header.parent_hash().to_owned();
        let mut timestamps: Vec<u64> = Vec::with_capacity(count as usize);
        for _ in 0..count {
            let header = match self.get_header(&block_hash) {
                Some(h) => h,
                None => break,
            };
            timestamps.push(header.timestamp());
            block_hash = header.parent_hash().to_owned();
        }
        timestamps
    }
}
