use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_types::{packed, prelude::*};

use crate::{prelude::*, LightClientProtocol, Status};

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
        let consensus = self.protocol.shared.consensus();
        let mmr_activated_number = consensus.hardfork_switch().mmr_activated_number();

        let active_chain = self.protocol.shared.active_chain();
        let last_hash = active_chain.tip_hash();
        let last_block = active_chain
            .get_block(&last_hash)
            .expect("checked: tip block should be existed");
        let last_header = last_block.header();
        let uncles_hash = last_block.calc_uncles_hash();
        let extension = last_block.extension();
        let last_header = packed::VerifiableHeader::new_builder()
            .header(last_header.data())
            .uncles_hash(uncles_hash)
            .extension(Pack::pack(&extension))
            .build();
        let total_difficulty = active_chain
            .get_block_ext(&last_hash)
            .map(|block_ext| block_ext.total_difficulty)
            .expect("checked: tip block should have block ext");

        let content = packed::SendLastState::new_builder()
            .mmr_activated_number(mmr_activated_number.pack())
            .last_header(last_header)
            .total_difficulty(total_difficulty.pack())
            .build();
        let message = packed::LightClientMessage::new_builder()
            .set(content)
            .build();

        self.nc.reply(self.peer, &message)
    }
}
