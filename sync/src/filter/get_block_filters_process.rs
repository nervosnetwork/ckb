use crate::filter::BlockFilter;
use crate::utils::send_message_to;
use crate::{attempt, Status};
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_store::ChainStore;
use ckb_types::core::BlockNumber;
use ckb_types::{packed, prelude::*};
use std::sync::Arc;

const BATCH_SIZE: BlockNumber = 100;

pub struct GetBlockFiltersProcess<'a> {
    message: packed::GetBlockFiltersReader<'a>,
    filter: &'a BlockFilter,
    nc: Arc<dyn CKBProtocolContext>,
    peer: PeerIndex,
}

impl<'a> GetBlockFiltersProcess<'a> {
    pub fn new(
        message: packed::GetBlockFiltersReader<'a>,
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
        if tip_number >= start_number {
            let mut block_hashes = Vec::new();
            let mut filters = Vec::new();
            for block_number in start_number..start_number + BATCH_SIZE {
                if let Some(block_hash) = snapshot.get_block_hash(block_number) {
                    if let Some(block_filter) = snapshot.get_block_filter(&block_hash) {
                        block_hashes.push(block_hash);
                        filters.push(block_filter);
                    } else {
                        break;
                    }
                } else {
                    break;
                }
            }
            let content = packed::BlockFilters::new_builder()
                .block_hashes(block_hashes.pack())
                .filters(filters.pack())
                .build();

            let message = packed::BlockFilterMessage::new_builder()
                .set(content)
                .build();
            attempt!(send_message_to(self.nc.as_ref(), self.peer, &message));
        }

        Status::ok()
    }
}
