use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_types::{packed, prelude::*, utilities::compact_to_difficulty, U256};

use crate::{prelude::*, LightClientProtocol, Status, StatusCode};

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

        let (tip_header, root) = match self.protocol.get_tip_state() {
            Ok(tip_state) => tip_state,
            Err(errmsg) => {
                return StatusCode::InternalError.with_context(errmsg);
            }
        };

        let total_difficulty = {
            let parent_total_difficulty: U256 = root.total_difficulty().unpack();
            let block_compact_target: u32 = tip_header.header().raw().compact_target().unpack();
            let block_difficulty = compact_to_difficulty(block_compact_target);
            parent_total_difficulty + block_difficulty
        };

        let content = packed::SendLastState::new_builder()
            .tip_header(tip_header)
            .total_difficulty(total_difficulty.pack())
            .build();
        let message = packed::LightClientMessage::new_builder()
            .set(content)
            .build();

        self.nc.reply(self.peer, &message)
    }
}
