use crate::synchronizer::Synchronizer;
use crate::{NetworkProtocol, Status, StatusCode, MAX_LOCATOR_SIZE};
use ckb_logger::{debug, info};
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_types::{
    core,
    packed::{self, Byte32},
    prelude::*,
};

pub struct GetHeadersProcess<'a> {
    message: packed::GetHeadersReader<'a>,
    synchronizer: &'a Synchronizer,
    peer: PeerIndex,
    nc: &'a dyn CKBProtocolContext,
}

impl<'a> GetHeadersProcess<'a> {
    pub fn new(
        message: packed::GetHeadersReader<'a>,
        synchronizer: &'a Synchronizer,
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
        if active_chain.is_initial_block_download() {
            info!(
                "Ignoring getheaders from peer={} because node is in initial block download",
                self.peer
            );
            self.send_in_ibd();
            return Status::ignored();
        }

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
            let data = message.as_slice().into();
            if let Err(err) = self.nc.send_message_to(self.peer, data) {
                return StatusCode::Network
                    .with_context(format!("Send SendHeaders error: {:?}", err,));
            }
        } else {
            return StatusCode::GetHeadersMissCommonAncestors
                .with_context(format!("{:#x?}", block_locator_hashes,));
        }
        Status::ok()
    }

    fn send_in_ibd(&self) {
        let content = packed::InIBD::new_builder().build();
        let message = packed::SyncMessage::new_builder().set(content).build();
        let data = message.as_slice().into();
        if let Err(err) = self
            .nc
            .send_message(NetworkProtocol::SYNC.into(), self.peer, data)
        {
            debug!("synchronizer send in ibd error: {:?}", err);
        }
    }
}
