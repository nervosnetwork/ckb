use crate::network::NetworkState;
use p2p::{
    service::{ServiceError, ServiceEvent, SessionType, TargetProtocol, TargetSession},
    traits::ServiceProtocol,
    ProtocolId, SessionId,
};
use std::{sync::Arc, time::Duration};

pub use p2p::{
    bytes::Bytes,
    context::{ProtocolContext, SessionContext},
    service::ServiceAsyncControl,
};

pub fn new_discovery_service_proto(
    network_state: NetworkState,
    discovery_local_address: bool,
    announce_check_interval: Option<Duration>,
) -> Box<dyn ServiceProtocol> {
    use crate::protocols::discovery;
    let addr_mgr = discovery::DiscoveryAddressManager {
        network_state: Arc::new(network_state),
        discovery_local_address: discovery_local_address,
    };
    Box::new(discovery::DiscoveryProtocol::new(
        addr_mgr,
        announce_check_interval,
    ))
}

pub fn new_idencovery_service_proto(
    network_state: NetworkState,
    flags: crate::protocols::identify::Flags,
) -> Box<dyn ServiceProtocol> {
    use crate::protocols::identify;
    let identify_callback = identify::IdentifyCallback::new(
        Arc::new(network_state),
        String::new(), // name
        String::new(), // client_version
        flags,
    );
    Box::new(identify::IdentifyProtocol::new(identify_callback))
}

pub fn new_disconnect_msg_service_proto(network_state: NetworkState) -> Box<dyn ServiceProtocol> {
    use crate::protocols::disconnect_message;
    Box::new(disconnect_message::DisconnectMessageProtocol::new(
        Arc::new(network_state),
    ))
}

pub fn new_feeler_service_proto(network_state: NetworkState) -> Box<dyn ServiceProtocol> {
    use crate::protocols::feeler;
    Box::new(feeler::Feeler::new(Arc::new(network_state)))
}

pub fn new_ping_service_proto(
    network_state: NetworkState,
    interval: Duration,
    timeout: Duration,
) -> Box<dyn ServiceProtocol> {
    use crate::protocols::ping;
    Box::new(ping::PingHandler::new(interval, timeout, Arc::new(network_state)).0)
}
