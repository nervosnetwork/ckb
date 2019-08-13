use crate::synchronizer::Synchronizer;
use crate::{attempt, NetworkProtocol, Status, StatusCode, MAX_LOCATOR_SIZE};
use ckb_core::header::Header;
use ckb_logger::debug;
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_protocol::{cast, GetHeaders, SyncMessage};
use failure::Error as FailureError;
use flatbuffers::FlatBufferBuilder;
use numext_fixed_hash::H256;
use std::convert::TryInto;

pub struct GetHeadersProcess<'a> {
    message: &'a GetHeaders<'a>,
    synchronizer: &'a Synchronizer,
    peer: PeerIndex,
    nc: &'a CKBProtocolContext,
}

impl<'a> GetHeadersProcess<'a> {
    pub fn new(
        message: &'a GetHeaders,
        synchronizer: &'a Synchronizer,
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

    pub fn execute(self) -> Status {
        if self.synchronizer.shared.is_initial_block_download() {
            self.send_in_ibd();
            return StatusCode::InitialBlockDownload
                .with_context("Ignore GetHeaders because node is in InitialBlockDownload");
        }

        let locator = attempt!(cast!(self.message.block_locator_hashes()));
        let locator_size = locator.len();
        if locator_size > MAX_LOCATOR_SIZE {
            return StatusCode::MalformedProtocolMessage
                .with_context(format!("GetHeaders locator hashes size {}", locator_size,));
        }

        let hash_stop = H256::zero(); // TODO PENDING self.message.hash_stop().into();
        let block_locator_hashes = locator
            .iter()
            .map(TryInto::try_into)
            .collect::<Result<Vec<_>, FailureError>>();
        let block_locator_hashes: Vec<H256> = attempt!(block_locator_hashes);

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
                return StatusCode::Network.with_context(format!("send Headers error: {:?}", err));
            }
        } else {
            let hashes = block_locator_hashes
                .iter()
                .map(|hash| format!(" {:#x}", hash))
                .collect::<String>();
            return StatusCode::MissingCommonAncestor.with_context(format!(
                "GetHeaders has not common ancestor with us: {}",
                hashes,
            ));
        }
        Status::ok()
    }

    fn send_in_ibd(&self) {
        let fbb = &mut FlatBufferBuilder::new();
        let message = SyncMessage::build_in_ibd(fbb);
        fbb.finish(message, None);

        if let Err(err) = self.nc.send_message(
            NetworkProtocol::SYNC.into(),
            self.peer,
            fbb.finished_data().into(),
        ) {
            debug!("synchronizer send in ibd error: {:?}", err);
        }
    }
}
