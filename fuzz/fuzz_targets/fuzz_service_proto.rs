#![no_main]
use libfuzzer_sys::fuzz_target;

// Note
//  If you want to use this fuzz, need to replace tentacle and related dependencies in Cargo.toml.
// [replace]
// "tentacle:0.4.2" = {path = '../../tentacle/tentacle'}
// "tentacle-multiaddr:0.3.4" = {path = '../../tentacle/multiaddr'}
// "tentacle-secio:0.5.7" = {path = '../../tentacle/secio'}

use ckb_network::{
    virtual_p2p::{
        channel, Bytes, ProtocolContext, ProtocolId, ServiceContext, ServiceProtocol,
        SessionContext,
    },
    NetworkState,
};
use std::task::Poll;
use std::{
    collections::HashMap,
    sync::{atomic::AtomicBool, Arc},
};
use tokio::time::{self, Duration};

use ckb_fuzz::BufManager;

struct ServiceProtoTest {
    data: BufManager,
    service_protocol: Box<dyn ServiceProtocol>,
    _channel_id: usize,
}

impl ServiceProtoTest {
    fn new(data: &[u8]) -> Result<Self, ()> {
        let mut data = BufManager::new(&data);

        use ckb_app_config::NetworkConfig;

        let mut network_config = NetworkConfig::default();
        network_config.path = std::path::PathBuf::from("./");
        let network_state = NetworkState::from_config(network_config).unwrap();

        let t = data.get::<u8>() % 5;

        let service_protocol = match t {
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
            4 => ckb_network::virtual_p2p::new_ping_service_proto(
                network_state,
                data.get(),
                data.get(),
            ),
            _ => return Err(()),
        };

        Ok(Self {
            data,
            service_protocol,
            _channel_id: 0,
        })
    }

    async fn run(mut self) -> Poll<()> {
        let (sender, _receiver) = channel(0);
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

        self.service_protocol
            .connected(proto_ctx.as_mut(&session_ctx), "")
            .await;

        self.service_protocol
            .received(
                proto_ctx.as_mut(&session_ctx),
                Bytes::from(self.data.other()),
            )
            .await;

        self.service_protocol
            .disconnected(proto_ctx.as_mut(&session_ctx))
            .await;

        Poll::Ready(())
    }
}

fuzz_target!(|data: &[u8]| {
    let t = ServiceProtoTest::new(data);
    if t.is_err() {
        return;
    }
    let t = t.unwrap();

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();

    let _r = rt.block_on(async move {
        tokio::select! {
            _ = t.run() => println!(""),
            _ = time::sleep(Duration::from_millis(100)) => println!("Timeout or Poll::Pending"),

        }
    });

    println!("---- mk0");
});
