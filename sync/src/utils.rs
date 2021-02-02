use crate::{Status, StatusCode};
use ckb_metrics::metrics;
use ckb_network::{CKBProtocolContext, PeerIndex, ProtocolId, SupportProtocols};
use ckb_types::packed::{RelayMessageReader, SyncMessageReader};
use ckb_types::prelude::*;

/// Send network message into parameterized `protocol_id` protocol connection.
///
/// Equal to `nc.send_message`.
#[must_use]
pub(crate) fn send_message<Message: Entity>(
    protocol_id: ProtocolId,
    nc: &dyn CKBProtocolContext,
    peer_index: PeerIndex,
    message: &Message,
) -> Status {
    let name = message_name(protocol_id, message);
    ckb_logger::trace!("nc.send_message {}", name);

    if let Err(err) = nc.send_message(protocol_id, peer_index, message.as_bytes()) {
        let error_message = format!("nc.send_message {}, error: {:?}", name, err);
        ckb_logger::error!("{}", error_message);
        return StatusCode::Network.with_context(error_message);
    }

    let bytes = message.as_bytes().len() as u64;
    metrics!(counter, "ckb.messages_bytes", bytes, "direction" => "out", "name" => name);
    Status::ok()
}

/// Send network message into `nc.protocol_id()` protocol connection.
///
/// Equal to `nc.send_message_to`.
#[must_use]
pub(crate) fn send_message_to<Message: Entity>(
    nc: &dyn CKBProtocolContext,
    peer_index: PeerIndex,
    message: &Message,
) -> Status {
    let protocol_id = nc.protocol_id();
    send_message(protocol_id, nc, peer_index, message)
}

// As for Sync protocol and Relay protocol, returns the internal item name;
// otherwise returns the entity name.
fn message_name<Message: Entity>(protocol_id: ProtocolId, message: &Message) -> String {
    if protocol_id == SupportProtocols::Sync.protocol_id() {
        SyncMessageReader::new_unchecked(message.as_slice())
            .to_enum()
            .item_name()
            .to_owned()
    } else if protocol_id == SupportProtocols::Relay.protocol_id() {
        RelayMessageReader::new_unchecked(message.as_slice())
            .to_enum()
            .item_name()
            .to_owned()
    } else {
        Message::NAME.to_owned()
    }
}
