use crate::synchronizer::Synchronizer;
use crate::{MAX_LOCATOR_SIZE, SYNC_USELESS_BAN_TIME};
use ckb_core::header::Header;
use ckb_logger::{debug, info, warn};
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_protocol::{cast, GetHeaders, SyncMessage};
use ckb_store::ChainStore;
use failure::Error as FailureError;
use flatbuffers::FlatBufferBuilder;
use numext_fixed_hash::H256;
use std::convert::TryInto;

pub struct GetHeadersProcess<'a, CS: ChainStore + 'a> {
    message: &'a GetHeaders<'a>,
    synchronizer: &'a Synchronizer<CS>,
    peer: PeerIndex,
    nc: &'a CKBProtocolContext,
}

impl<'a, CS> GetHeadersProcess<'a, CS>
where
    CS: ChainStore + 'a,
{
    pub fn new(
        message: &'a GetHeaders,
        synchronizer: &'a Synchronizer<CS>,
        peer: PeerIndex,
        nc: &'a CKBProtocolContext,
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
            return Ok(());
        }

        let locator = cast!(self.message.block_locator_hashes())?;
        let locator_size = locator.len();
        if locator_size > MAX_LOCATOR_SIZE {
            warn!(
                " getheaders locator size {} from peer={}",
                locator_size, self.peer
            );
            cast!(None)?;
        }

        let hash_stop = H256::zero(); // TODO PENDING self.message.hash_stop().into();
        let block_locator_hashes = locator
            .iter()
            .map(TryInto::try_into)
            .collect::<Result<Vec<_>, FailureError>>()?;

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
            let headers: Vec<Header> = self
                .synchronizer
                .shared
                .get_locator_response(block_number, &hash_stop);
            // response headers

            debug!("headers len={}", headers.len());

            let fbb = &mut FlatBufferBuilder::new();
            let message = SyncMessage::build_headers(fbb, &headers);
            fbb.finish(message, None);
            if let Err(err) = self
                .nc
                .send_message_to(self.peer, fbb.finished_data().into())
            {
                debug!("synchronizer send Headers error: {:?}", err);
            }
        } else {
            for hash in &block_locator_hashes[..] {
                warn!("unknown block headers from peer {} {:#x}", self.peer, hash);
            }
            // Got 'headers' message without known blocks
            self.nc.ban_peer(self.peer, SYNC_USELESS_BAN_TIME);
        }
        Ok(())
    }
}
