use crate::ckb_protocol::CKBProtocol;
use crate::ckb_protocol_handler::CKBProtocolHandler;
use crate::ckb_protocol_handler::{CKBProtocolContext, DefaultCKBProtocolContext};
use crate::network::Network;
use crate::NetworkConfig;
use crate::{Error, ErrorKind, ProtocolId};
use ckb_util::Mutex;
use futures::future::Future;
use futures::sync::oneshot;
use libp2p::{Multiaddr, PeerId};
use log::{debug, info};
use std::sync::Arc;
use std::thread;
use tokio::runtime;

pub struct StopHandler {
    signal: oneshot::Sender<()>,
    thread: thread::JoinHandle<()>,
}

impl StopHandler {
    pub fn new(signal: oneshot::Sender<()>, thread: thread::JoinHandle<()>) -> StopHandler {
        StopHandler { signal, thread }
    }

    pub fn close(self) {
        let StopHandler { signal, thread } = self;
        if let Err(e) = signal.send(()) {
            debug!(target: "network", "send shutdown signal error, ignoring error: {:?}", e)
        };
        thread.join().expect("join network_service thread");
    }
}

pub struct NetworkService {
    network: Arc<Network>,
    stop_handler: Mutex<Option<StopHandler>>,
}

impl NetworkService {
    #[inline]
    pub fn external_urls(&self, max_urls: usize) -> Vec<(String, u8)> {
        self.network.external_urls(max_urls)
    }

    #[inline]
    pub fn node_id(&self) -> String {
        self.network.node_id()
    }

    pub fn with_protocol_context<F, T>(&self, protocol_id: ProtocolId, f: F) -> Option<T>
    where
        F: FnOnce(&CKBProtocolContext) -> T,
    {
        match self.network.ckb_protocols.find_protocol(protocol_id) {
            Some(_) => Some(f(&DefaultCKBProtocolContext::new(
                Arc::clone(&self.network),
                protocol_id,
            ))),
            None => None,
        }
    }

    pub fn run_in_thread(
        config: &NetworkConfig,
        ckb_protocols: Vec<CKBProtocol<Arc<CKBProtocolHandler>>>,
    ) -> Result<NetworkService, Error> {
        let network = Network::inner_build(config, ckb_protocols)?;
        let (close_tx, close_rx) = oneshot::channel();
        let (init_tx, init_rx) = oneshot::channel();
        let join_handle = thread::spawn({
            let network = Arc::clone(&network);
            let config = config.clone();
            move || {
                info!(
                    target: "network",
                    "network peer_id {:?}",
                    network.local_public_key().clone().into_peer_id()
                );
                let network_future =
                    Network::build_network_future(network, &config, close_rx).unwrap();
                init_tx.send(()).unwrap();
                // here we use default config
                let network_runtime = runtime::Runtime::new().unwrap();
                match network_runtime.block_on_all(network_future) {
                    Ok(_) => info!(target: "network", "network service exit"),
                    Err(err) => panic!("network service exit unexpected {}", err),
                }
            }
        });
        init_rx.wait().map_err(|err| {
            Error::from(ErrorKind::Other(
                format!("initialize network service error: {}", err.to_string()).to_owned(),
            ))
        })?;
        Ok(NetworkService {
            network,
            stop_handler: Mutex::new(Some(StopHandler::new(close_tx, join_handle))),
        })
    }

    // Send shutdown signal to server
    // This method will not wait for the server stopped, you should use server_future or
    // thread_handle to achieve that.
    pub fn close(&self) {
        let handler = self
            .stop_handler
            .lock()
            .take()
            .expect("network_service can only close once");
        handler.close();
    }

    pub fn add_node(&self, peer_id: &PeerId, address: Multiaddr) {
        self.network.add_node(peer_id, address)
    }
}
