use crate::ServiceContext;
use futures::sync::mpsc::UnboundedSender;
use log::{error, info, trace, warn};
use p2p::{
    service::{ProtocolEvent, ServiceError, ServiceEvent},
    traits::ServiceHandle,
};

pub enum NetworkEvent {
    Protocol(ProtocolEvent),
    Event(ServiceEvent),
    Error(ServiceError),
}
pub struct EventHandler {
    sender: UnboundedSender<NetworkEvent>,
}

impl EventHandler {
    pub fn new(sender: UnboundedSender<NetworkEvent>) -> Self {
        EventHandler { sender }
    }
}

impl ServiceHandle for EventHandler {
    fn handle_error(&mut self, _context: &mut ServiceContext, error: ServiceError) {
        warn!(target: "network", "p2p service error: {:?}", error);
        match self.sender.unbounded_send(NetworkEvent::Error(error)) {
            Ok(_) => {
                trace!(target: "network", "send network error success");
            }
            Err(err) => error!(target: "network", "send network error failed: {:?}", err),
        }
    }

    fn handle_event(&mut self, _context: &mut ServiceContext, event: ServiceEvent) {
        info!(target: "network", "p2p service event: {:?}", event);
        match self.sender.unbounded_send(NetworkEvent::Event(event)) {
            Ok(_) => {
                trace!(target: "network", "send network service event success");
            }
            Err(err) => error!(target: "network", "send network event failed: {:?}", err),
        }
    }

    fn handle_proto(&mut self, _context: &mut ServiceContext, event: ProtocolEvent) {
        match self.sender.unbounded_send(NetworkEvent::Protocol(event)) {
            Ok(_) => {
                trace!(target: "network", "send network protocol event success");
            }
            Err(err) => error!(target: "network", "send network event failed: {:?}", err),
        }
    }
}
