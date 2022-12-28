use crate::mock_sync::mock_synchronizer::MockSynchronizer;
use ckb_constant::sync::MAX_LOCATOR_SIZE;
use ckb_logger::debug;
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_sync::{attempt, utils::send_message, Status, StatusCode};
use ckb_types::{
    core,
    packed::{self, Byte32},
    prelude::*,
};

pub struct GetHeadersProcess<'a> {
    message: packed::GetHeadersReader<'a>,
    synchronizer: &'a MockSynchronizer,
    peer: PeerIndex,
    nc: &'a dyn CKBProtocolContext,
}

impl<'a> GetHeadersProcess<'a> {
    pub fn new(
        message: packed::GetHeadersReader<'a>,
        synchronizer: &'a MockSynchronizer,
        peer: PeerIndex,
        nc: &'a dyn CKBProtocolContext,
    ) -> Self {
        GetHeadersProcess {
            message,
            nc,
            synchronizer,
            peer,
        }
    }

    pub fn execute(self) -> Status {
        let active_chain = self.synchronizer.shared.active_chain();

        let block_locator_hashes = self
            .message
            .block_locator_hashes()
            .iter()
            .map(|x| x.to_entity())
            .collect::<Vec<Byte32>>();
        let hash_stop = self.message.hash_stop().to_entity();
        let locator_size = block_locator_hashes.len();
        if locator_size > MAX_LOCATOR_SIZE {
            return StatusCode::ProtocolMessageIsMalformed.with_context(format!(
                "Locator count({}) > MAX_LOCATOR_SIZE({})",
                locator_size, MAX_LOCATOR_SIZE,
            ));
        }

        if let Some(block_number) =
            active_chain.locate_latest_common_block(&hash_stop, &block_locator_hashes[..])
        {
            debug!(
                "headers latest_common={} tip={} begin",
                block_number,
                active_chain.tip_header().number(),
            );

            self.synchronizer.peers().getheaders_received(self.peer);
            let headers: Vec<core::HeaderView> =
                active_chain.get_locator_response(block_number, &hash_stop);
            // response headers

            debug!("headers len={}", headers.len());

            let content = packed::SendHeaders::new_builder()
                .headers(headers.into_iter().map(|x| x.data()).pack())
                .build();
            let message = packed::SyncMessage::new_builder().set(content).build();

            attempt!(send_message(
                self.nc.protocol_id(),
                self.nc,
                self.peer,
                &message
            ));
        } else {
            return StatusCode::GetHeadersMissCommonAncestors
                .with_context(format!("{:#x?}", block_locator_hashes,));
        }
        Status::ok()
    }
}
