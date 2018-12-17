use libp2p::core::{transport::BoxedMuxed, upgrade, Multiaddr, PeerId, Transport};
use libp2p::{self, mplex, secio, yamux, TransportTimeout};
use std::time::Duration;
use std::usize;
use tokio::io::{AsyncRead, AsyncWrite};

pub fn new_transport(
    local_private_key: secio::SecioKeyPair,
    timeout: Duration,
) -> BoxedMuxed<(PeerId, impl AsyncRead + AsyncWrite)> {
    let mut mplex_config = mplex::MplexConfig::new();
    mplex_config.max_buffer_len_behaviour(mplex::MaxBufferBehaviour::Block);
    mplex_config.max_buffer_len(usize::MAX);

    let transport = libp2p::CommonTransport::new()
        .with_upgrade(secio::SecioConfig {
            key: local_private_key,
        })
        .and_then(move |out, endpoint, client_addr| {
            let key = out.remote_key;
            let upgrade = upgrade::map(yamux::Config::default(), move |muxer| (key, muxer));
            upgrade::apply(out.stream, upgrade, endpoint, client_addr)
            // TODO: check key
        })
        .into_connection_reuse()
        .map(|(key, substream), _| (key.into_peer_id(), substream));

    let transport = TransportTimeout::new(transport, timeout);
    transport.boxed_muxed()
}

pub struct TransportOutput<S> {
    pub socket: S,
    pub peer_id: PeerId,
    pub original_addr: Multiaddr,
}
