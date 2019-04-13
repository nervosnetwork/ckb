use super::compact_block::CompactBlock;
use crate::relayer::Relayer;
use ckb_core::{header::Header, BlockNumber};
use ckb_network::{CKBProtocolContext, SessionId};
use ckb_protocol::{CompactBlock as FbsCompactBlock, RelayMessage};
use ckb_shared::shared::Shared;
use ckb_shared::store::ChainStore;
use ckb_traits::{BlockMedianTimeContext, ChainProvider};
use ckb_verification::{HeaderResolverWrapper, HeaderVerifier, Verifier};
use failure::Error as FailureError;
use flatbuffers::FlatBufferBuilder;
use fnv::FnvHashMap;
use log::warn;
use numext_fixed_hash::H256;
use std::convert::TryInto;
use std::sync::Arc;

pub struct CompactBlockProcess<'a, CS> {
    message: &'a FbsCompactBlock<'a>,
    relayer: &'a Relayer<CS>,
    peer: SessionId,
    nc: &'a mut CKBProtocolContext,
}

impl<'a, CS: ChainStore> CompactBlockProcess<'a, CS> {
    pub fn new(
        message: &'a FbsCompactBlock,
        relayer: &'a Relayer<CS>,
        peer: SessionId,
        nc: &'a mut CKBProtocolContext,
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
        let block_hash = compact_block.header.hash();
        let mut missing_indexes: Vec<usize> = Vec::new();
        {
            let mut pending_compact_blocks = self.relayer.state.pending_compact_blocks.lock();
            if pending_compact_blocks.get(&block_hash).is_none()
                && self.relayer.get_block(&block_hash).is_none()
            {
                let resolver =
                    HeaderResolverWrapper::new(&compact_block.header, self.relayer.shared.clone());
                let header_verifier = HeaderVerifier::new(
                    CompactBlockMedianTimeView {
                        header: &compact_block.header,
                        pending_compact_blocks: &pending_compact_blocks,
                        shared: &self.relayer.shared,
                    },
                    Arc::clone(&self.relayer.shared.consensus().pow_engine()),
                );

                if header_verifier.verify(&resolver).is_ok() {
                    let ret = {
                        let chain_state = self.relayer.shared.chain_state().lock();
                        self.relayer.request_proposal_txs(
                            &chain_state,
                            self.nc,
                            self.peer,
                            &compact_block,
                        );
                        self.relayer
                            .reconstruct_block(&chain_state, &compact_block, Vec::new())
                    };
                    match ret {
                        Ok(block) => {
                            self.relayer
                                .accept_block(self.nc, self.peer, &Arc::new(block))
                        }
                        Err(missing) => {
                            missing_indexes = missing;
                            pending_compact_blocks
                                .insert(block_hash.clone(), compact_block.clone());
                        }
                    }
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
            let ret = self.nc.send(self.peer, fbb.finished_data().to_vec());

            if ret.is_err() {
                warn!(target: "relay", "CompactBlockProcess relay error {:?}", ret);
            }
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
