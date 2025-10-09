use crate::filter::BlockFilter;
use crate::utils::send_message_to;
use crate::{Status, attempt};
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_types::core::BlockNumber;
use ckb_types::{packed, prelude::*};
use std::sync::Arc;

const BATCH_SIZE: BlockNumber = 1000;

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
        let active_chain = self.filter.shared.active_chain();
        let start_number: BlockNumber = self.message.to_entity().start_number().into();
        let latest: BlockNumber = active_chain.get_latest_built_filter_block_number();

        if latest >= start_number {
            let mut block_hashes = Vec::new();
            let mut filters = Vec::new();
            'outer: for block_number in start_number..start_number + BATCH_SIZE {
                if let Some(block_hash) = active_chain.get_block_hash(block_number) {
                    if let Some(block_filter) = active_chain.get_block_filter(&block_hash) {
                        block_hashes.push(block_hash);
                        filters.push(block_filter);
                        if packed::BlockFilters::new_builder()
                            .filters(filters.clone())
                            .build()
                            .filters()
                            .as_slice()
                            .len()
                            >= (1.8 * 1024.0 * 1024.0) as usize
                        {
                            // Break if the encoded size of `filters` reaches 1.8MB, to avoid frame size too large
                            block_hashes.pop();
                            filters.pop();
                            break 'outer;
                        }
                    } else {
                        break;
                    }
                } else {
                    break;
                }
            }
            let content = packed::BlockFilters::new_builder()
                .start_number(start_number)
                .block_hashes(block_hashes)
                .filters(filters)
                .build();

            let message = packed::BlockFilterMessage::new_builder()
                .set(content)
                .build();
            attempt!(send_message_to(self.nc.as_ref(), self.peer, &message))
        } else {
            Status::ignored()
        }
    }
}
