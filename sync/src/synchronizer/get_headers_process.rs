use bigint::H256;
use ckb_chain::chain::ChainProvider;
use ckb_chain::PowEngine;
use ckb_protocol::{
    build_header_args, GetHeaders, Header, Headers, HeadersArgs, SyncMessage, SyncMessageArgs,
    SyncPayload,
};
use core::header::IndexedHeader;
use flatbuffers::FlatBufferBuilder;
use network::{NetworkContext, PeerId};
use synchronizer::Synchronizer;

pub struct GetHeadersProcess<'a, C: 'a, P: 'a> {
    message: &'a GetHeaders<'a>,
    synchronizer: &'a Synchronizer<C, P>,
    peer: PeerId,
    nc: &'a NetworkContext,
}

impl<'a, C, P> GetHeadersProcess<'a, C, P>
where
    C: ChainProvider + 'a,
    P: PowEngine + 'a,
{
    pub fn new(
        message: &'a GetHeaders,
        synchronizer: &'a Synchronizer<C, P>,
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
        let block_locator_hashes = self
            .message
            .block_locator_hashes()
            .unwrap()
            .chunks(32)
            .map(H256::from)
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

            let builder = &mut FlatBufferBuilder::new();
            {
                let vec = headers
                    .iter()
                    .map(|header| {
                        let header_args = build_header_args(builder, header);
                        Header::create(builder, &header_args)
                    }).collect::<Vec<_>>();
                let headers = Some(builder.create_vector(&vec));
                let payload =
                    Some(Headers::create(builder, &HeadersArgs { headers }).as_union_value());
                let payload_type = SyncPayload::Headers;
                let message = SyncMessage::create(
                    builder,
                    &SyncMessageArgs {
                        payload_type,
                        payload,
                    },
                );
                builder.finish(message, None);
            }

            self.nc.respond(0, builder.finished_data().to_vec());
        } else {
            warn!(target: "sync", "\n\nunknown block headers from peer {} {:#?}\n\n", self.peer, block_locator_hashes);
            // Got 'headers' message without known blocks
            // ban or close peers
        }
    }
}
