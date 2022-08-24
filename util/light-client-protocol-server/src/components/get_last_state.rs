use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_types::{packed, prelude::*};

use crate::{LightClientProtocol, Status};

pub(crate) struct GetLastStateProcess<'a> {
    message: packed::GetLastStateReader<'a>,
    protocol: &'a LightClientProtocol,
    peer: PeerIndex,
    nc: &'a dyn CKBProtocolContext,
}

impl<'a> GetLastStateProcess<'a> {
    pub(crate) fn new(
        message: packed::GetLastStateReader<'a>,
        protocol: &'a LightClientProtocol,
        peer: PeerIndex,
        nc: &'a dyn CKBProtocolContext,
    ) -> Self {
        Self {
            message,
            protocol,
            peer,
            nc,
        }
    }

    pub(crate) fn execute(self) -> Status {
        let subscribe: bool = self.message.subscribe().unpack();
        if subscribe {
            self.nc.with_peer_mut(
                self.peer,
                Box::new(|peer| {
                    peer.if_lightclient_subscribed = true;
                }),
            );
        }

        self.protocol.send_last_state(self.nc, self.peer)
    }
}
