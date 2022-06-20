use ckb_merkle_mountain_range::{leaf_index_to_mmr_size, leaf_index_to_pos};
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_types::{packed, prelude::*, utilities::merkle_mountain_range::ChainRootMMR};

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
        let active_chain = self.protocol.shared.active_chain();
        let snapshot = self.protocol.shared.shared().snapshot();

        let last_hash = self.message.last_hash().to_entity();
        let last_number = match active_chain.get_block_header(&last_hash) {
            Some(header) => header.number(),
            None => {
                return StatusCode::InvalidLastBlock
                    .with_context("last_hash is not on active chain".to_owned());
            }
        };

        let tip_hash = active_chain.tip_hash();
        let tip_block = active_chain
            .get_block(&tip_hash)
            .expect("checked: tip block should be existed");
        let tip_header = tip_block.header();
        let tip_number = tip_header.number();
        let uncles_hash = tip_block.calc_uncles_hash();
        let extension = tip_block.extension();
        let tip_header = packed::VerifiableHeader::new_builder()
            .header(tip_header.data())
            .uncles_hash(uncles_hash)
            .extension(Pack::pack(&extension))
            .build();
        let total_difficulty = active_chain
            .get_block_ext(&tip_hash)
            .map(|block_ext| block_ext.total_difficulty)
            .expect("checked: tip block should have block ext");

        let positions = vec![leaf_index_to_pos(last_number)];
        let (chain_root, proof) = {
            let mmr_size = leaf_index_to_mmr_size(tip_number - 1);
            let mmr = ChainRootMMR::new(mmr_size, &**snapshot);
            let root = match mmr.get_root() {
                Ok(root) => root,
                Err(err) => {
                    let errmsg = format!("failed to generate a root since {:?}", err);
                    return StatusCode::InternalError.with_context(errmsg);
                }
            };
            let proof = match mmr.gen_proof(positions) {
                Ok(proof) => proof,
                Err(err) => {
                    let errmsg = format!("failed to generate a proof since {:?}", err);
                    return StatusCode::InternalError.with_context(errmsg);
                }
            };
            (root, proof)
        };

        let content = packed::SendLastState::new_builder()
            .tip_header(tip_header)
            .total_difficulty(total_difficulty.pack())
            .chain_root(chain_root)
            .proof(proof.pack())
            .build();
        let message = packed::LightClientMessage::new_builder()
            .set(content)
            .build();

        self.nc.reply(self.peer, &message)
    }
}
