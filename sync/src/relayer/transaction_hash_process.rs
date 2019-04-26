use crate::relayer::Relayer;
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_protocol::RelayTransactionHash as FbsRelayTransactionHash;
use ckb_store::ChainStore;
use failure::Error as FailureError;
use log::debug;
use numext_fixed_hash::H256;
use std::convert::TryInto;

pub struct TransactionHashProcess<'a, CS> {
    message: &'a FbsRelayTransactionHash<'a>,
    relayer: &'a Relayer<CS>,
    _nc: &'a CKBProtocolContext,
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
            _nc: nc,
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
        } else {
            debug!(
                target: "relay",
                "transaction({:#x}) from {} not known, get it from the peer",
                tx_hash,
                self.peer,
            );
            let last_ask_timeout = self
                .relayer
                .state
                .tx_already_asked
                .lock()
                .get(&tx_hash)
                .cloned();
            if let Some(next_ask_timeout) = self
                .relayer
                .peers
                .state
                .write()
                .get_mut(&self.peer)
                .and_then(|peer_state| peer_state.add_ask_for_tx(tx_hash.clone(), last_ask_timeout))
            {
                self.relayer
                    .state
                    .tx_already_asked
                    .lock()
                    .insert(tx_hash.clone(), next_ask_timeout);
            }
        }

        Ok(())
    }
}
