use crate::ckb_protocol::CKBProtocol;
use crate::ckb_protocol_handler::CKBProtocolHandler;
use crate::ckb_protocol_handler::{CKBProtocolContext, DefaultCKBProtocolContext};
use crate::network::Network;
use crate::peer_store::PeerStore;
use crate::peers_registry::PeerConnection;
use crate::NetworkConfig;
use crate::{Error, ErrorKind, ProtocolId};
use ckb_util::RwLock;
use futures::future::Future;
use futures::sync::oneshot;
use libp2p::core::PeerId;
use log::{debug, info};
use std::boxed::Box;
use std::io::{Error as IoError, ErrorKind as IoErrorKind};
use std::sync::Arc;
use std::thread;
use tokio::runtime;

pub struct NetworkService {
    network: Arc<Network>,
    close_tx: Option<oneshot::Sender<()>>,
    join_handle: Option<thread::JoinHandle<()>>,
}

impl Drop for NetworkService {
    fn drop(&mut self) {
        self.shutdown().expect("shutdown CKB network service");
    }
}

impl NetworkService {
    #[inline]
    pub fn external_url(&self) -> Option<String> {
        self.network.external_url()
    }

    #[allow(dead_code)]
    #[inline]
    pub(crate) fn peer_store<'a>(&'a self) -> &'a RwLock<Box<PeerStore>> {
        &self.network.peer_store()
    }

    #[inline]
    pub fn add_peer(&self, peer_id: PeerId, peer: PeerConnection) {
        self.network.add_peer(peer_id, peer);
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
            join_handle: Some(join_handle),
            close_tx: Some(close_tx),
        })
    }

    // Send shutdown signal to server
    // This method will not wait for the server stopped, you should use server_future or
    // thread_handle to achieve that.
    fn shutdown(&mut self) -> Result<(), IoError> {
        debug!(target: "network", "shutdown network service self: {:?}", self.external_url());
        if let Some(close_tx) = self.close_tx.take() {
            let _ = close_tx
                .send(())
                .map_err(|err| debug!(target: "network", "send shutdown signal error, ignoring error: {:?}", err));
        };
        if let Some(join_handle) = self.join_handle.take() {
            join_handle.join().map_err(|_| {
                IoError::new(IoErrorKind::Other, "can't join network_service thread")
            })?
        }
        Ok(())
    }
}
