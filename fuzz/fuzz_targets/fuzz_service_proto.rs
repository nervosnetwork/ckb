#![no_main]

use libfuzzer_sys::fuzz_target;

use ckb_network::{
    virtual_p2p::{Bytes, ProtocolContext, SessionContext},
    NetworkState,
};

use ckb_fuzz::BufManager;

fuzz_target!(|data: &[u8]| {
    let mut data = BufManager::new(data);

    use ckb_app_config::NetworkConfig;

    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();

    let mut network_config = NetworkConfig::default();
    network_config.path = std::path::PathBuf::from("./");
    let network_state = NetworkState::from_config(network_config).unwrap();

    let t: u8 = data.get();

    let mut service_protocol = match t {
        0 => {
            let announce_check_interval = if data.get::<bool>() {
                Some(data.get())
            } else {
                None
            };
            ckb_network::virtual_p2p::new_discovery_service_proto(
                network_state,
                data.get(),
                announce_check_interval,
            )
        }
        1 => ckb_network::virtual_p2p::new_idencovery_service_proto(network_state, data.get()),
        2 => ckb_network::virtual_p2p::new_disconnect_msg_service_proto(network_state),
        3 => ckb_network::virtual_p2p::new_feeler_service_proto(network_state),
        4 => {
            ckb_network::virtual_p2p::new_ping_service_proto(network_state, data.get(), data.get())
        }
        _ => return,
    };

    let mut proto_ctx = ProtocolContext::default();
    let session_ctx = SessionContext::default();

    rt.block_on(async {
        service_protocol.init(&mut proto_ctx).await;

        service_protocol
            .connected(proto_ctx.as_mut(&session_ctx), "")
            .await;

        service_protocol
            .received(proto_ctx.as_mut(&session_ctx), Bytes::from(data.other()))
            .await;

        service_protocol
            .disconnected(proto_ctx.as_mut(&session_ctx))
            .await;
    });
});
