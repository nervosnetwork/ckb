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
        let snapshot = self.synchronizer.shared.snapshot();
        if snapshot.is_initial_block_download() {
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
            return Err(err_msg(
                "locator size is greater than MAX_LOCATOR_SIZE".to_owned(),
            ));
        }

        if let Some(block_number) =
            snapshot.locate_latest_common_block(&hash_stop, &block_locator_hashes[..])
        {
            debug!(
                "headers latest_common={} tip={} begin",
                block_number,
                snapshot.tip_header().number(),
            );

            self.synchronizer.peers().getheaders_received(self.peer);
            let header_ctxs: Vec<core::HeaderContext> = snapshot.get_locator_response(
                block_number,
                &hash_stop,
                self.synchronizer.shared().consensus().header_context_type(),
            );
            // response headers

            debug!("headers len={}", header_ctxs.len());

            // build send header message
            let message = if self.synchronizer.shared().consensus().pow.is_poa() {
                let content = packed::SendPOAHeaders::new_builder()
                    .headers(
                        header_ctxs
                            .into_iter()
                            .map(|header_ctx| header_ctx.into())
                            .pack(),
                    )
                    .build();
                packed::SyncMessage::new_builder().set(content).build()
            } else {
                let content = packed::SendHeaders::new_builder()
                    .headers(
                        header_ctxs
                            .into_iter()
                            .map(|header_ctx| header_ctx.header().data())
                            .pack(),
                    )
                    .build();
                packed::SyncMessage::new_builder().set(content).build()
            };

            let data = message.as_slice().into();
            if let Err(err) = self.nc.send_message_to(self.peer, data) {
                debug!("synchronizer send Headers error: {:?}", err);
            }
        } else {
            for hash in &block_locator_hashes[..] {
                warn!("unknown block headers from peer {} {}", self.peer, hash);
            }
            // Got 'headers' message without known blocks
            self.nc.ban_peer(
                self.peer,
                SYNC_USELESS_BAN_TIME,
                String::from("send us headers with unknown-block"),
            );
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
