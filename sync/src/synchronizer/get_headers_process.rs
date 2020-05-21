use crate::synchronizer::Synchronizer;
use crate::utils::{send_inibd, send_sendheaders};
use crate::{Status, StatusCode, MAX_LOCATOR_SIZE};
use ckb_logger::{debug, error, info};
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_types::{
    core,
    packed::{self, Byte32},
    prelude::*,
};

pub struct GetHeadersProcess<'a> {
    block_locator_hashes: Vec<Byte32>,
    hash_stop: Byte32,
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
        let block_locator_hashes = message
            .block_locator_hashes()
            .iter()
            .map(|x| x.to_entity())
            .collect::<Vec<Byte32>>();
        let hash_stop = message.hash_stop().to_entity();
        GetHeadersProcess {
            block_locator_hashes,
            hash_stop,
            nc,
            synchronizer,
            peer,
        }
    }

    pub fn execute(self) -> Status {
        {
            fail::fail_point!("recv_getheaders", |_| {
                debug!(
                    "[failpoint] recv_getheaders({:?} from {}",
                    self.block_locator_hashes, self.peer
                );
                Status::ignored()
            })
        }

        let active_chain = self.synchronizer.shared.active_chain();
        let hash_stop = &self.hash_stop;
        let locator_size = self.block_locator_hashes.len();
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
            let state = self.synchronizer.shared.state();
            if let Some(flag) = state.peers().get_flag(self.peer) {
                if flag.is_outbound || flag.is_whitelist || flag.is_protect {
                    state.insert_peer_unknown_header_list(self.peer, self.block_locator_hashes);
                }
            };
            return Status::ignored();
        }

        if let Some(block_number) =
            active_chain.locate_latest_common_block(&hash_stop, &self.block_locator_hashes[..])
        {
            debug!(
                "headers latest_common={} tip={} begin",
                block_number,
                active_chain.tip_header().number(),
            );

            self.synchronizer.peers().getheaders_received(self.peer);
            let headers: Vec<core::HeaderView> =
                active_chain.get_locator_response(block_number, &hash_stop);

            if let Err(err) = send_sendheaders(
                self.nc,
                self.peer,
                headers.into_iter().map(|h| h.data()).collect(),
            ) {
                return StatusCode::Network
                    .with_context(format!("send_sendheaders error: {:?}", err));
            }
        } else {
            return StatusCode::GetHeadersMissCommonAncestors
                .with_context(format!("{:#x?}", self.block_locator_hashes));
        }
        Status::ok()
    }

    fn send_in_ibd(&self) {
        if let Err(err) = send_inibd(self.nc, self.peer) {
            error!("send_inibd error: {:?}", err);
        }
    }
}
