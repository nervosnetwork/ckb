use crate::synchronizer::Synchronizer;
use crate::{NetworkProtocol, MAX_LOCATOR_SIZE, SYNC_USELESS_BAN_TIME};
use ckb_logger::{debug, info, warn};
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_types::{
    core,
    packed::{self, Byte32},
    prelude::*,
};
use failure::{err_msg, Error as FailureError};

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

    pub fn execute(self) -> Result<(), FailureError> {
        if self.synchronizer.shared.is_initial_block_download() {
            info!(
                "Ignoring getheaders from peer={} because node is in initial block download",
                self.peer
            );
            self.send_in_ibd();
            return Ok(());
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
            warn!(
                " getheaders locator size {} from peer={}",
                locator_size, self.peer
            );
            Err(err_msg(
                "locator size is greater than MAX_LOCATOR_SIZE".to_owned(),
            ))?;
        }

        if let Some(block_number) = self
            .synchronizer
            .shared
            .locate_latest_common_block(&hash_stop, &block_locator_hashes[..])
        {
            debug!(
                "headers latest_common={} tip={} begin",
                block_number,
                self.synchronizer.shared.tip_header().number(),
            );

            self.synchronizer.peers().getheaders_received(self.peer);
            let headers: Vec<core::HeaderView> = self
                .synchronizer
                .shared
                .get_locator_response(block_number, &hash_stop);
            // response headers

            debug!("headers len={}", headers.len());

            let content = packed::SendHeaders::new_builder()
                .headers(headers.into_iter().map(|x| x.data()).pack())
                .build();
            let message = packed::SyncMessage::new_builder().set(content).build();
            let data = message.as_slice().into();
            if let Err(err) = self.nc.send_message_to(self.peer, data) {
                debug!("synchronizer send Headers error: {:?}", err);
            }
        } else {
            for hash in &block_locator_hashes[..] {
                warn!("unknown block headers from peer {} {}", self.peer, hash);
            }
            // Got 'headers' message without known blocks
            self.nc.ban_peer(self.peer, SYNC_USELESS_BAN_TIME);
        }
        Ok(())
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
