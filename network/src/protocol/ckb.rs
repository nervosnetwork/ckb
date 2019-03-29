use crate::{
    peer_store::{Behaviour, Status},
    peers_registry::RegisterResult,
    protocol::ckb_handler::DefaultCKBProtocolContext,
    CKBProtocolHandler, NetworkState, ProtocolContext, ProtocolContextMutRef,
};
use bytes::Bytes;
use log::{debug, error, info};
use p2p::{
    builder::MetaBuilder,
    service::{ProtocolHandle, ProtocolMeta},
    traits::ServiceProtocol,
    ProtocolId,
};
use std::sync::Arc;
use std::time::Instant;

pub type ProtocolVersion = u8;

pub struct CKBProtocol {
    id: ProtocolId,
    // for example: b"/ckb/"
    protocol_name: String,
    // supported version, used to check protocol version
    supported_versions: Vec<ProtocolVersion>,
    handler: Box<dyn CKBProtocolHandler + Send + 'static>,
    network_state: Arc<NetworkState>,
}

impl CKBProtocol {
    pub fn new(
        protocol_name: String,
        id: ProtocolId,
        versions: &[ProtocolVersion],
        handler: Box<dyn CKBProtocolHandler + Send + 'static>,
        network_state: Arc<NetworkState>,
    ) -> Self {
        CKBProtocol {
            id,
            handler,
            network_state,
            protocol_name: format!("/ckb/{}/", protocol_name).to_string(),
            supported_versions: {
                let mut versions: Vec<_> = versions.to_vec();
                versions.sort_by(|a, b| b.cmp(a));
                versions.to_vec()
            },
        }
    }

    pub fn id(&self) -> ProtocolId {
        self.id
    }

    pub fn protocol_name(&self) -> String {
        self.protocol_name.clone()
    }

    pub fn match_version(&self, version: ProtocolVersion) -> bool {
        self.supported_versions.contains(&version)
    }

    pub fn build(self) -> ProtocolMeta {
        let protocol_name = self.protocol_name();
        let supported_versions = self
            .supported_versions
            .iter()
            .map(|v| v.to_string())
            .collect::<Vec<_>>();
        MetaBuilder::default()
            .id(self.id)
            .name(move |_| protocol_name.clone())
            .support_versions(supported_versions)
            .service_handle(move || {
                let handler = CKBHandler::new(self.id, self.network_state, self.handler);
                ProtocolHandle::Callback(Box::new(handler))
            })
            .build()
    }
}

struct CKBHandler {
    id: ProtocolId,
    network_state: Arc<NetworkState>,
    handler: Box<dyn CKBProtocolHandler + Send + 'static>,
}

impl CKBHandler {
    pub fn new(
        id: ProtocolId,
        network_state: Arc<NetworkState>,
        handler: Box<dyn CKBProtocolHandler + Send + 'static>,
    ) -> CKBHandler {
        CKBHandler {
            id,
            network_state,
            handler,
        }
    }
}

impl ServiceProtocol for CKBHandler {
    fn init(&mut self, context: &mut ProtocolContext) {
        let context = Box::new(DefaultCKBProtocolContext::new(
            self.id,
            Arc::clone(&self.network_state),
            context.control().clone(),
        ));
        self.handler.initialize(context);
    }

    fn connected(&mut self, mut context: ProtocolContextMutRef, version: &str) {
        let network = &self.network_state;
        let session = context.session;
        let (peer_id, version) = {
            // TODO: version number should be discussed.
            let parsed_version = version.parse::<u8>().ok();
            if session.remote_pubkey.is_none() || parsed_version.is_none() {
                error!(
                    target: "network",
                    "ckb protocol connected error, addr: {}, protocol:{}, version: {}",
                    session.address,
                    self.id,
                    version,
                );
                context.disconnect(session.id);
                return;
            }
            (
                session
                    .remote_pubkey
                    .as_ref()
                    .map(|pubkey| pubkey.peer_id())
                    .expect("remote_pubkey existence checked"),
                parsed_version.expect("parsed_version existence checked"),
            )
        };
        debug!(
            target: "network",
            "ckb protocol connected, addr: {}, protocol: {}, version: {}, peer_id: {:?}",
            session.address,
            self.id,
            version,
            &peer_id,
        );

        match network.accept_connection(
            peer_id.clone(),
            session.address.clone(),
            session.id,
            session.ty.into(),
            self.id,
            version,
        ) {
            Ok(register_result) => {
                // update status in peer_store
                if let RegisterResult::New(_) = register_result {
                    let mut peer_store = network.peer_store().write();
                    peer_store.report(&peer_id, Behaviour::Connect);
                    peer_store.update_status(&peer_id, Status::Connected);
                }
                // call handler
                self.handler.connected(
                    Box::new(DefaultCKBProtocolContext::new(
                        self.id,
                        Arc::clone(&self.network_state),
                        context.control().clone(),
                    )),
                    register_result.peer_index(),
                )
            }
            Err(err) => {
                network.drop_peer(&peer_id);
                context.disconnect(session.id);
                info!(
                    target: "network",
                    "reject connection from {} {}, because {:?}",
                    peer_id.to_base58(),
                    session.address,
                    err,
                )
            }
        }
    }

    fn disconnected(&mut self, mut context: ProtocolContextMutRef) {
        let session = context.session;
        if let Some(peer_id) = session
            .remote_pubkey
            .as_ref()
            .map(|pubkey| pubkey.peer_id())
        {
            debug!(
                target: "network",
                "ckb protocol disconnect, addr: {}, protocol: {}, peer_id: {:?}",
                session.address,
                self.id,
                &peer_id,
            );

            let network = &self.network_state;
            // update disconnect in peer_store
            {
                let mut peer_store = network.peer_store().write();
                peer_store.report(&peer_id, Behaviour::UnexpectedDisconnect);
                peer_store.update_status(&peer_id, Status::Disconnected);
            }
            if let Some(peer_index) = network.get_peer_index(&peer_id) {
                // call handler
                self.handler.disconnected(
                    Box::new(DefaultCKBProtocolContext::new(
                        self.id,
                        Arc::clone(network),
                        context.control().clone(),
                    )),
                    peer_index,
                );
            }
            // disconnect
            network.drop_peer(&peer_id);
        }
    }

    fn received(&mut self, mut context: ProtocolContextMutRef, data: Bytes) {
        let session = context.session;
        if let Some((peer_id, _peer_index)) = session
            .remote_pubkey
            .as_ref()
            .map(|pubkey| pubkey.peer_id())
            .and_then(|peer_id| {
                self.network_state
                    .get_peer_index(&peer_id)
                    .map(|peer_index| (peer_id, peer_index))
            })
        {
            debug!(
                target: "network",
                "ckb protocol received, addr: {}, protocol: {}, peer_id: {:?}",
                session.address,
                self.id,
                &peer_id,
            );

            let now = Instant::now();
            let network = &self.network_state;
            network.modify_peer(&peer_id, |peer| {
                peer.last_message_time = Some(now);
            });
            let peer_index = network
                .get_peer_index(&peer_id)
                .expect("get peer index failed");
            self.handler.received(
                Box::new(DefaultCKBProtocolContext::new(
                    self.id,
                    Arc::clone(network),
                    context.control().clone(),
                )),
                peer_index,
                data,
            )
        } else {
            error!(target: "network", "can not get peer_id");
        }
    }
    fn notify(&mut self, context: &mut ProtocolContext, token: u64) {
        let context = Box::new(DefaultCKBProtocolContext::new(
            self.id,
            Arc::clone(&self.network_state),
            context.control().clone(),
        ));
        self.handler.timer_triggered(context, token);
    }
}
