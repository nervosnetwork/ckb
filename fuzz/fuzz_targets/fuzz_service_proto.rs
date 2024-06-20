#![no_main]
use libfuzzer_sys::fuzz_target;

// Note
//  If you want to use this fuzz, need to replace tentacle and related dependencies in Cargo.toml.
// [replace]
// "tentacle:0.4.2" = {path = '../../tentacle/tentacle'}
// "tentacle-multiaddr:0.3.4" = {path = '../../tentacle/multiaddr'}
// "tentacle-secio:0.5.7" = {path = '../../tentacle/secio'}

use ckb_network::{
    virtual_p2p::{channel, Bytes, ProtocolContext, ProtocolId, ServiceContext, SessionContext},
    NetworkState,
};
use std::{
    collections::HashMap,
    sync::{atomic::AtomicBool, Arc},
};

use ckb_fuzz::BufManager;

fuzz_target!(|data: &[u8]| {
    let (sender, mut _recv) = channel(0);
    let data = data.to_vec();

    {
        use ckb_app_config::NetworkConfig;

        // let rt = tokio::runtime::Builder::new_current_thread()
        //     .build()
        //     .unwrap();

        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async move {
            let mut data = BufManager::new(&data);

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
                1 => ckb_network::virtual_p2p::new_idencovery_service_proto(
                    network_state,
                    data.get(),
                ),
                2 => ckb_network::virtual_p2p::new_disconnect_msg_service_proto(network_state),
                3 => ckb_network::virtual_p2p::new_feeler_service_proto(network_state),
                4 => ckb_network::virtual_p2p::new_ping_service_proto(
                    network_state,
                    data.get(),
                    data.get(),
                ),
                _ => return,
            };

            let session_ctx = SessionContext::default();
            let mut proto_ctx = ProtocolContext::new(
                ServiceContext::new(
                    sender,
                    HashMap::new(),
                    None,
                    Arc::new(AtomicBool::default()),
                ),
                ProtocolId::default(),
            );

            tokio::spawn(async move {
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
    }
});
