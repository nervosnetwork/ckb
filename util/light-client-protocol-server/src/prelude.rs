use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_types::{packed, prelude::*};

use crate::{Status, StatusCode};

pub(crate) trait LightClientProtocolReply<'a> {
    fn reply(&'a self, peer_index: PeerIndex, message: &packed::LightClientMessage) -> Status;
}

impl<'a> LightClientProtocolReply<'a> for &(dyn CKBProtocolContext + 'a) {
    fn reply(&'a self, peer_index: PeerIndex, message: &packed::LightClientMessage) -> Status {
        let enum_message = message.to_enum();
        let item_name = enum_message.item_name();
        let protocol_id = self.protocol_id();
        if let Err(err) = self.send_message(protocol_id, peer_index, message.as_bytes()) {
            let error_message = format!("nc.send_message {} failed since {:?}", item_name, err);
            StatusCode::Network.with_context(error_message)
        } else {
            Status::ok()
        }
    }
}
