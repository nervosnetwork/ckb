use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_types::packed;

use crate::{LightClientProtocol, Status};

pub(crate) struct GetLastStateProcess<'a> {
    _message: packed::GetLastStateReader<'a>,
    protocol: &'a LightClientProtocol,
    peer: PeerIndex,
    nc: &'a dyn CKBProtocolContext,
}

impl<'a> GetLastStateProcess<'a> {
    pub(crate) fn new(
        _message: packed::GetLastStateReader<'a>,
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
        self.nc.with_peer_mut(
            self.peer,
            Box::new(|peer| {
                peer.is_lightclient = true;
            }),
        );

        self.protocol.send_last_state(self.nc, self.peer)
    }
}
