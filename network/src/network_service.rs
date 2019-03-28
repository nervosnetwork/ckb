use crate::protocol::ckb_handler::{CKBProtocolContext, DefaultCKBProtocolContext};
use crate::{errors::Error, CKBEvent, NetworkConfig, ProtocolId};
use crate::{
    multiaddr::Multiaddr,
    network::{CKBProtocols, Network},
    PeerId,
};
use ckb_util::Mutex;
use futures::future::Future;
use futures::sync::mpsc::Receiver;
use futures::sync::oneshot;
use log::{debug, error, info};
use std::sync::Arc;
use tokio::runtime;

pub struct StopHandler {
    signal: oneshot::Sender<()>,
    network_runtime: runtime::Runtime,
}

impl StopHandler {
    pub fn new(signal: oneshot::Sender<()>, network_runtime: runtime::Runtime) -> StopHandler {
        StopHandler {
            signal,
            network_runtime,
        }
    }

    pub fn close(self) {
        let StopHandler {
            signal,
            network_runtime,
        } = self;
        if let Err(e) = signal.send(()) {
            debug!(target: "network", "send shutdown signal error, ignoring error: {:?}", e)
        };
        // TODO: not that gracefully shutdown, will output below error message:
        //       "terminate called after throwing an instance of 'std::system_error'"
        network_runtime.shutdown_now();
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
        match self.network.find_protocol_without_version(protocol_id) {
            Some(_) => Some(f(&DefaultCKBProtocolContext::new(
                Arc::clone(&self.network),
                protocol_id,
            ))),
            None => None,
        }
    }

    pub fn run_in_thread(
        config: &NetworkConfig,
        ckb_protocols: CKBProtocols,
        ckb_event_receiver: Receiver<CKBEvent>,
    ) -> Result<NetworkService, Error> {
        let (network, p2p_service, timer_registry, receivers) =
            Network::inner_build(config, ckb_protocols)?;
        let (close_tx, close_rx) = oneshot::channel();
        let (init_tx, init_rx) = oneshot::channel();

        info!(
            target: "network",
            "network peer_id {:?}",
            network.local_public_key().peer_id()
        );
        let network_future = Network::build_network_future(
            Arc::clone(&network),
            &config,
            close_rx,
            p2p_service,
            timer_registry,
            ckb_event_receiver,
            receivers,
        )
        .expect("Network thread init");
        init_tx.send(()).expect("Network init signal send");
        // here we use default config
        let mut network_runtime = runtime::Runtime::new().expect("Network tokio runtime init");
        network_runtime.spawn(
            network_future
                .map(|_| info!(target: "network", "network service exit"))
                .map_err(|err| error!("network service exit unexpected {}", err)),
        );

        init_rx.wait().map_err(|_err| Error::Shutdown)?;
        Ok(NetworkService {
            network,
            stop_handler: Mutex::new(Some(StopHandler::new(close_tx, network_runtime))),
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
