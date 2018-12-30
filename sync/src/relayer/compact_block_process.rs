use super::compact_block::CompactBlock;
use crate::relayer::Relayer;
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_protocol::{CompactBlock as FbsCompactBlock, RelayMessage};
use ckb_shared::block_median_time_context::BlockMedianTimeContext;
use ckb_shared::index::ChainIndex;
use ckb_shared::shared::ChainProvider;
use ckb_util::RwLockUpgradableReadGuard;
use ckb_verification::{HeaderResolverWrapper, HeaderVerifier, Verifier};
use flatbuffers::FlatBufferBuilder;
use numext_fixed_hash::H256;
use std::sync::Arc;

pub struct CompactBlockProcess<'a, CI: ChainIndex + 'a> {
    message: &'a FbsCompactBlock<'a>,
    relayer: &'a Relayer<CI>,
    peer: PeerIndex,
    nc: &'a CKBProtocolContext,
}

impl<'a, CI> CompactBlockProcess<'a, CI>
where
    CI: ChainIndex + 'static,
{
    pub fn new(
        message: &'a FbsCompactBlock,
        relayer: &'a Relayer<CI>,
        peer: PeerIndex,
        nc: &'a CKBProtocolContext,
    ) -> Self {
        CompactBlockProcess {
            message,
            nc,
            relayer,
            peer,
        }
    }

    pub fn execute(self) {
        let compact_block: CompactBlock = (*self.message).into();
        let block_hash = compact_block.header.hash();
        let pending_compact_blocks = self.relayer.state.pending_compact_blocks.upgradable_read();
        if pending_compact_blocks.get(&block_hash).is_none()
            && self.relayer.get_block(&block_hash).is_none()
        {
            let resolver =
                HeaderResolverWrapper::new(&compact_block.header, self.relayer.shared.clone());
            let header_verifier = HeaderVerifier::new(
                CompactBlockMedianTimeView {
                    relayer: self.relayer,
                },
                Arc::clone(&self.relayer.shared.consensus().pow_engine()),
            );

            if header_verifier.verify(&resolver).is_ok() {
                self.relayer
                    .request_proposal_txs(self.nc, self.peer, &compact_block);

                match self.relayer.reconstruct_block(&compact_block, Vec::new()) {
                    (Some(block), _) => {
                        self.relayer
                            .accept_block(self.nc, self.peer, &Arc::new(block))
                    }
                    (None, missing_indexes) => {
                        {
                            let mut write_guard =
                                RwLockUpgradableReadGuard::upgrade(pending_compact_blocks);
                            write_guard.insert(block_hash.clone(), compact_block.clone());
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
                        let _ = self.nc.send(self.peer, fbb.finished_data().to_vec());
                    }
                }
            }
        }
    }
}

struct CompactBlockMedianTimeView<'a, CI>
where
    CI: ChainIndex + 'static,
{
    relayer: &'a Relayer<CI>,
}

impl<'a, CI> ::std::clone::Clone for CompactBlockMedianTimeView<'a, CI>
where
    CI: ChainIndex + 'static,
{
    fn clone(&self) -> Self {
        CompactBlockMedianTimeView {
            relayer: self.relayer,
        }
    }
}

impl<'a, CI> BlockMedianTimeContext for CompactBlockMedianTimeView<'a, CI>
where
    CI: ChainIndex + 'static,
{
    fn block_count(&self) -> u32 {
        self.relayer.shared.consensus().median_time_block_count() as u32
    }
    fn timestamp(&self, hash: &H256) -> Option<u64> {
        self.relayer
            .state
            .pending_compact_blocks
            .read()
            .get(hash)
            .map(|cb| cb.header.timestamp())
            .or_else(|| {
                self.relayer
                    .shared
                    .block_header(hash)
                    .map(|header| header.timestamp())
            })
    }
    fn parent_hash(&self, hash: &H256) -> Option<H256> {
        self.relayer
            .state
            .pending_compact_blocks
            .read()
            .get(hash)
            .map(|cb| cb.header.parent_hash().to_owned())
            .or_else(|| {
                self.relayer
                    .shared
                    .block_header(hash)
                    .map(|header| header.parent_hash().to_owned())
            })
    }
}
