use crate::synchronizer::Synchronizer;
use crate::utils::{send_message, send_message_to};
use crate::{attempt, Status, StatusCode};
use ckb_constant::sync::MAX_LOCATOR_SIZE;
use ckb_logger::{debug, info};
use ckb_network::{CKBProtocolContext, PeerIndex, SupportProtocols};
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

        if active_chain.is_initial_block_download() {
            info!(
                "Ignoring getheaders from peer={} because node is in initial block download",
                self.peer
            );
            self.send_in_ibd();
            let shared = self.synchronizer.shared().shared();
            let state = self.synchronizer.shared().state();
            if let Some(flag) = state.peers().get_flag(self.peer) {
                if flag.is_outbound || flag.is_whitelist || flag.is_protect {
                    state.insert_peer_unknown_header_list(shared, self.peer, block_locator_hashes);
                }
            };
            return Status::ignored();
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

            attempt!(send_message_to(self.nc, self.peer, &message));
        } else {
            return StatusCode::GetHeadersMissCommonAncestors
                .with_context(format!("{:#x?}", block_locator_hashes,));
        }
        Status::ok()
    }

    fn send_in_ibd(&self) {
        let content = packed::InIBD::new_builder().build();
        let message = packed::SyncMessage::new_builder().set(content).build();
        let _ignore = send_message(
            SupportProtocols::Sync.protocol_id(),
            self.nc,
            self.peer,
            &message,
        );
    }
}
