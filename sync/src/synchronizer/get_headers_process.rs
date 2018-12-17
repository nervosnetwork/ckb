use crate::synchronizer::Synchronizer;
use crate::MAX_LOCATOR_SIZE;
use ckb_core::header::Header;
use ckb_network::{CKBProtocolContext, PeerIndex, Severity};
use ckb_protocol::{FlatbuffersVectorIterator, GetHeaders, SyncMessage};
use ckb_shared::index::ChainIndex;
use flatbuffers::FlatBufferBuilder;
use log::{debug, info, warn};
use numext_fixed_hash::H256;

pub struct GetHeadersProcess<'a, CI: ChainIndex + 'a> {
    message: &'a GetHeaders<'a>,
    synchronizer: &'a Synchronizer<CI>,
    peer: PeerIndex,
    nc: &'a CKBProtocolContext,
}

impl<'a, CI> GetHeadersProcess<'a, CI>
where
    CI: ChainIndex + 'a,
{
    pub fn new(
        message: &'a GetHeaders,
        synchronizer: &'a Synchronizer<CI>,
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

    pub fn execute(self) {
        if self.synchronizer.is_initial_block_download() {
            info!(target: "sync", "Ignoring getheaders from peer={} because node is in initial block download", self.peer);
            return;
        }
        if let Some(locator) = self.message.block_locator_hashes() {
            let locator_size = locator.len();
            if locator_size > MAX_LOCATOR_SIZE {
                warn!(target: "sync", " getheaders locator size {} from peer={}", locator_size, self.peer);
                self.nc
                    .report_peer(self.peer, Severity::Bad("over maximum locator size"));
                return;
            }

            let hash_stop = H256::zero(); // TODO PENDING self.message.hash_stop().unwrap().into();
            let block_locator_hashes = FlatbuffersVectorIterator::new(locator)
                .map(|bytes| H256::from_slice(bytes.seq().unwrap()).unwrap())
                .collect::<Vec<_>>();

            if let Some(block_number) = self
                .synchronizer
                .locate_latest_common_block(&hash_stop, &block_locator_hashes[..])
            {
                debug!(target: "sync", "\n\nheaders latest_common={} tip={} begin\n\n", block_number, {self.synchronizer.tip_header().number()});

                self.synchronizer.peers.getheaders_received(self.peer);
                let headers: Vec<Header> = self
                    .synchronizer
                    .get_locator_response(block_number, &hash_stop);
                // response headers

                debug!(target: "sync", "\nheaders len={}\n", headers.len());

                let fbb = &mut FlatBufferBuilder::new();
                let message = SyncMessage::build_headers(fbb, &headers);
                fbb.finish(message, None);
                let _ = self.nc.send(self.peer, fbb.finished_data().to_vec());
            } else {
                warn!(target: "sync", "\n\nunknown block headers from peer {} {:?}\n\n", self.peer, block_locator_hashes);
                // Got 'headers' message without known blocks
                // ban or close peers
                self.nc
                    .report_peer(self.peer, Severity::Bad("without common headers"));
            }
        }
    }
}
