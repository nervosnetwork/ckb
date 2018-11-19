use bigint::H256;
use ckb_chain::chain::ChainProvider;
use ckb_protocol;
use core::header::IndexedHeader;
use network::NetworkContextExt;
use network::{NetworkContext, PeerId};
use protobuf::RepeatedField;
use synchronizer::Synchronizer;

pub struct GetHeadersProcess<'a, C: 'a> {
    message: &'a ckb_protocol::GetHeaders,
    synchronizer: &'a Synchronizer<C>,
    peer: PeerId,
    nc: &'a NetworkContext,
}

impl<'a, C> GetHeadersProcess<'a, C>
where
    C: ChainProvider + 'a,
{
    pub fn new(
        message: &'a ckb_protocol::GetHeaders,
        synchronizer: &'a Synchronizer<C>,
        peer: &PeerId,
        nc: &'a NetworkContext,
    ) -> Self {
        GetHeadersProcess {
            message,
            nc,
            synchronizer,
            peer: *peer,
        }
    }

    pub fn execute(self) {
        if self.synchronizer.is_initial_block_download() {
            info!(target: "sync", "Ignoring getheaders from peer={} because node is in initial block download", self.peer);
            return;
        }

        let hash_stop = H256::from_slice(self.message.get_hash_stop());
        let block_locator_hashes: Vec<H256> = self
            .message
            .get_block_locator_hashes()
            .iter()
            .map(|hash| H256::from_slice(&hash[..]))
            .collect();
        if let Some(block_number) = self
            .synchronizer
            .locate_latest_common_block(&hash_stop, &block_locator_hashes[..])
        {
            debug!(target: "sync", "\n\nheaders latest_common={} tip={} begin\n\n", block_number, {self.synchronizer.tip_header().number});

            self.synchronizer.peers.getheaders_received(&self.peer);
            let headers: Vec<IndexedHeader> = self
                .synchronizer
                .get_locator_response(block_number, &hash_stop);
            // response headers

            debug!(target: "sync", "\nheaders len={}\n", headers.len());
            let mut payload = ckb_protocol::Payload::new();
            let mut headers_proto = ckb_protocol::Headers::new();
            headers_proto.set_headers(RepeatedField::from_vec(
                headers.iter().map(|h| &h.header).map(Into::into).collect(),
            ));
            payload.set_headers(headers_proto);
            let _ = self.nc.respond_payload(payload);
            debug!(target: "sync", "\nrespond headers len={}\n", headers.len());
        } else {
            warn!(target: "sync", "\n\nunknown block headers from peer {} {:#?}\n\n", self.peer, block_locator_hashes);
            // Got 'headers' message without known blocks
            // ban or close peers
        }
    }
}
