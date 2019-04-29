use crate::relayer::Relayer;
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_protocol::{RelayMessage, RelayTransactionHash as FbsRelayTransactionHash};
use ckb_shared::store::ChainStore;
use failure::Error as FailureError;
use flatbuffers::FlatBufferBuilder;
use log::debug;
use numext_fixed_hash::H256;
use std::convert::TryInto;

const MAX_ASK_TX_TIME: u8 = 10;

pub struct TransactionHashProcess<'a, CS> {
    message: &'a FbsRelayTransactionHash<'a>,
    relayer: &'a Relayer<CS>,
    nc: &'a CKBProtocolContext,
    peer: PeerIndex,
}

impl<'a, CS: ChainStore> TransactionHashProcess<'a, CS> {
    pub fn new(
        message: &'a FbsRelayTransactionHash,
        relayer: &'a Relayer<CS>,
        nc: &'a CKBProtocolContext,
        peer: PeerIndex,
    ) -> Self {
        TransactionHashProcess {
            message,
            relayer,
            nc,
            peer,
        }
    }

    pub fn execute(self) -> Result<(), FailureError> {
        let tx_hash: H256 = (*self.message).try_into()?;
        if self.relayer.state.already_known(&tx_hash) {
            debug!(
                target: "relay",
                "transaction({:#x}) from {} already known, ignore it",
                tx_hash,
                self.peer,
            );
        } else if self.relayer.state.get_asked(&tx_hash) >= MAX_ASK_TX_TIME {
            debug!(
                target: "relay",
                "transaction({:#x}) from {}, already asked {} time, give up",
                tx_hash,
                self.peer,
                MAX_ASK_TX_TIME,
            );
        } else {
            debug!(
                target: "relay",
                "transaction({:#x}) from {} not known, get it from the peer",
                tx_hash,
                self.peer,
            );
            self.relayer.state.incr_asked(tx_hash.clone());

            let fbb = &mut FlatBufferBuilder::new();
            let message = RelayMessage::build_get_transaction(fbb, &tx_hash);
            fbb.finish(message, None);
            let data = fbb.finished_data().into();
            self.nc.send_message_to(self.peer, data);
        }

        Ok(())
    }
}
