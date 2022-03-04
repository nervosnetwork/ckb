use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_types::{packed, prelude::*};

use crate::{prelude::*, LightClientProtocol, Status};

pub(crate) struct GetChainInfoProcess<'a> {
    _message: packed::GetChainInfoReader<'a>,
    protocol: &'a LightClientProtocol,
    peer: PeerIndex,
    nc: &'a dyn CKBProtocolContext,
}

impl<'a> GetChainInfoProcess<'a> {
    pub(crate) fn new(
        _message: packed::GetChainInfoReader<'a>,
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
        let consensus = self.protocol.shared.consensus();
        let mmr_activated_number = consensus.mmr_activated_number();

        let content = packed::SendChainInfo::new_builder()
            .mmr_activated_number(mmr_activated_number.pack())
            .build();
        let message = packed::LightClientMessage::new_builder()
            .set(content)
            .build();

        self.nc.reply(self.peer, &message)
    }
}
