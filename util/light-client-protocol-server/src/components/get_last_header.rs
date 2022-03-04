use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_types::{packed, prelude::*};

use crate::{prelude::*, LightClientProtocol, Status};

pub(crate) struct GetLastHeaderProcess<'a> {
    _message: packed::GetLastHeaderReader<'a>,
    protocol: &'a LightClientProtocol,
    peer: PeerIndex,
    nc: &'a dyn CKBProtocolContext,
}

impl<'a> GetLastHeaderProcess<'a> {
    pub(crate) fn new(
        _message: packed::GetLastHeaderReader<'a>,
        protocol: &'a LightClientProtocol,
        peer: PeerIndex,
        nc: &'a dyn CKBProtocolContext,
    ) -> Self {
        Self {
            _message,
            protocol,
            peer,
            nc,
        }
    }

    pub(crate) fn execute(self) -> Status {
        let last_header = self.protocol.shared.active_chain().tip_header();

        let content = packed::SendLastHeader::new_builder()
            .header(last_header.data())
            .build();
        let message = packed::LightClientMessage::new_builder()
            .set(content)
            .build();

        self.nc.reply(self.peer, &message)
    }
}
