use crate::{Status, StatusCode};
use ckb_error::{Error as CKBError, ErrorKind, InternalError, InternalErrorKind};
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
    if let Err(err) = nc.send_message(protocol_id, peer_index, message.as_bytes()) {
        let name = message_name(protocol_id, message);
        let error_message = format!("nc.send_message {}, error: {:?}", name, err);
        ckb_logger::error!("{}", error_message);
        return StatusCode::Network.with_context(error_message);
    }

    let bytes = message.as_bytes().len() as u64;
    let item_id = item_id(protocol_id, message);
    metrics!(
        counter,
        "ckb.messages_bytes", bytes,
        "direction" => "out",
        "protocol_id" => protocol_id.value().to_string(),
        "item_id" => item_id.to_string(),
    );
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

// As for Sync protocol and Relay protocol, returns the internal item id;
// otherwise returns 0.
fn item_id<Message: Entity>(protocol_id: ProtocolId, message: &Message) -> u32 {
    if protocol_id == SupportProtocols::Sync.protocol_id() {
        SyncMessageReader::new_unchecked(message.as_slice()).item_id()
    } else if protocol_id == SupportProtocols::Relay.protocol_id() {
        RelayMessageReader::new_unchecked(message.as_slice()).item_id()
    } else {
        0
    }
}

/// return whether the error's kind is `InternalErrorKind::Database`
///
/// ### Panic
///
/// Panic if the error kind is `InternalErrorKind::DataCorrupted`.
/// If the database is corrupted, panic is better than handle it silently.
pub(crate) fn is_internal_db_error(error: &CKBError) -> bool {
    if error.kind() == ErrorKind::Internal {
        let error_kind = error
            .downcast_ref::<InternalError>()
            .expect("error kind checked")
            .kind();
        if error_kind == InternalErrorKind::DataCorrupted {
            panic!("{}", error)
        } else {
            return error_kind == InternalErrorKind::Database;
        }
    }
    false
}
