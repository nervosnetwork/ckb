use crate::filter::{block_filter_hash, BlockFilter};
use crate::utils::send_message_to;
use crate::{attempt, Status};
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_store::ChainStore;
use ckb_types::{core::BlockNumber, packed, prelude::*};
use std::sync::Arc;

const BATCH_SIZE: BlockNumber = 2000;

pub struct GetBlockFilterHashesProcess<'a> {
    message: packed::GetBlockFilterHashesReader<'a>,
    filter: &'a BlockFilter,
    nc: Arc<dyn CKBProtocolContext>,
    peer: PeerIndex,
}

impl<'a> GetBlockFilterHashesProcess<'a> {
    pub fn new(
        message: packed::GetBlockFilterHashesReader<'a>,
        filter: &'a BlockFilter,
        nc: Arc<dyn CKBProtocolContext>,
        peer: PeerIndex,
    ) -> Self {
        Self {
            message,
            nc,
            filter,
            peer,
        }
    }

    pub fn execute(self) -> Status {
        let snapshot = self.filter.shared.shared().snapshot();
        let start_number: BlockNumber = self.message.to_entity().start_number().unpack();
        let tip_number: BlockNumber = snapshot.get_tip_header().expect("tip stored").number();

        let mut parent_block_filter_hash = packed::Byte32::zero();
        let mut block_filter_hashes = Vec::new();

        if tip_number >= start_number {
            if start_number > 0 {
                if let Some(parent_block_hash) = snapshot.get_block_hash(start_number - 1) {
                    if let Some(parent_block_filter) = snapshot.get_block_filter(&parent_block_hash)
                    {
                        parent_block_filter_hash = block_filter_hash(parent_block_filter)
                    }
                }
            };

            for block_number in start_number..start_number + BATCH_SIZE {
                if let Some(block_hash) = snapshot.get_block_hash(block_number) {
                    if let Some(block_filter) = snapshot.get_block_filter(&block_hash) {
                        block_filter_hashes.push(block_filter_hash(block_filter));
                    } else {
                        break;
                    }
                } else {
                    break;
                }
            }
            let content = packed::BlockFilterHashes::new_builder()
                .start_number(start_number.pack())
                .parent_block_filter_hash(parent_block_filter_hash)
                .block_filter_hashes(block_filter_hashes.pack())
                .build();

            let message = packed::BlockFilterMessage::new_builder()
                .set(content)
                .build();
            attempt!(send_message_to(self.nc.as_ref(), self.peer, &message));
        }

        Status::ok()
    }
}
