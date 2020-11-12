use crate::{Status, StatusCode};
use ckb_metrics::metrics;
use ckb_network::{CKBProtocolContext, PeerIndex, ProtocolId, SupportProtocols};
use ckb_types::packed::{RelayMessage, SyncMessage};
use ckb_types::prelude::*;

pub(crate) fn send_message<Message: Entity>(
    protocol_id: ProtocolId,
    nc: &dyn CKBProtocolContext,
    peer_index: PeerIndex,
    message: &Message,
) -> Status {
    let name = message_name(protocol_id, message);
    if let Err(err) = nc.send_message(protocol_id, peer_index, message.as_bytes()) {
        ckb_logger::debug!(
            "nc.send_message failed, message name: {}, error: {:?}",
            name,
            err
        );
        return StatusCode::Network.with_context(format!("Send {}: {:?}", name, err));
    }

    let bytes = message.as_bytes().len() as u64;
    metrics!(counter, "ckb.messages_total", 1, "direction" => "out", "name" => name.to_owned());
    metrics!(counter, "ckb.messages_bytes", bytes, "direction" => "out", "name" => name);
    Status::ok()
}

pub(crate) fn send_message_to<Message: Entity>(
    nc: &dyn CKBProtocolContext,
    peer_index: PeerIndex,
    message: &Message,
) -> Status {
    let protocol_id = nc.protocol_id();
    send_message(protocol_id, nc, peer_index, message)
}

fn message_name<Message: Entity>(protocol_id: ProtocolId, message: &Message) -> String {
    if protocol_id == SupportProtocols::Sync.protocol_id() {
        SyncMessage::from_slice(message.as_slice())
            .expect("protocol_id match with message structure")
            .to_enum()
            .item_name()
            .to_owned()
    } else if protocol_id == SupportProtocols::Relay.protocol_id() {
        RelayMessage::from_slice(message.as_slice())
            .expect("protocol_id match with message structure")
            .to_enum()
            .item_name()
            .to_owned()
    } else {
        Message::NAME.to_owned()
    }
}
