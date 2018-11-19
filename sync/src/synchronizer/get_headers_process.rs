use bigint::H256;
use ckb_chain::chain::ChainProvider;
use ckb_protocol::{FlatbuffersVectorIterator, GetHeaders, SyncMessage};
use core::header::IndexedHeader;
use flatbuffers::FlatBufferBuilder;
use network::{NetworkContext, PeerId};
use synchronizer::Synchronizer;

pub struct GetHeadersProcess<'a, C: 'a> {
    message: &'a GetHeaders<'a>,
    synchronizer: &'a Synchronizer<C>,
    peer: PeerId,
    nc: &'a NetworkContext,
}

impl<'a, C> GetHeadersProcess<'a, C>
where
    C: ChainProvider + 'a,
{
    pub fn new(
        message: &'a GetHeaders,
        synchronizer: &'a Synchronizer<C>,
        peer: PeerId,
        nc: &'a NetworkContext,
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
        let hash_stop = H256::zero(); // TODO PENDING self.message.hash_stop().unwrap().into();
        let block_locator_hashes =
            FlatbuffersVectorIterator::new(self.message.block_locator_hashes().unwrap())
                .map(|bytes| H256::from_slice(bytes.seq().unwrap()))
                .collect::<Vec<_>>();

        if let Some(block_number) = self
            .synchronizer
            .locate_latest_common_block(&hash_stop, &block_locator_hashes[..])
        {
            debug!(target: "sync", "\n\nheaders latest_common={} tip={} begin\n\n", block_number, {self.synchronizer.tip_header().number});

            self.synchronizer.peers.getheaders_received(self.peer);
            let headers: Vec<IndexedHeader> = self
                .synchronizer
                .get_locator_response(block_number, &hash_stop);
            // response headers

            debug!(target: "sync", "\nheaders len={}\n", headers.len());

            let fbb = &mut FlatBufferBuilder::new();
            let message = SyncMessage::build_headers(fbb, &headers);
            fbb.finish(message, None);
            self.nc.respond(0, fbb.finished_data().to_vec());
        } else {
            warn!(target: "sync", "\n\nunknown block headers from peer {} {:#?}\n\n", self.peer, block_locator_hashes);
            // Got 'headers' message without known blocks
            // ban or close peers
        }
    }
}
