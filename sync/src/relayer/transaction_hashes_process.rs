use crate::relayer::Relayer;
use ckb_logger::debug_target;
use ckb_network::PeerIndex;
use ckb_types::{
    packed::{self, Byte32},
    prelude::*,
};
use failure::Error as FailureError;
use futures::future::Future;
use std::collections::HashMap;

pub struct TransactionHashesProcess<'a> {
    message: packed::RelayTransactionHashesReader<'a>,
    relayer: &'a Relayer,
    peer: PeerIndex,
}

impl<'a> TransactionHashesProcess<'a> {
    pub fn new(
        message: packed::RelayTransactionHashesReader<'a>,
        relayer: &'a Relayer,
        peer: PeerIndex,
    ) -> Self {
        TransactionHashesProcess {
            message,
            relayer,
            peer,
        }
    }

    pub fn execute(self) -> Result<(), FailureError> {
        let hashes: Vec<Byte32> = {
            let tx_filter = self.relayer.shared().tx_filter();
            self.message
                .tx_hashes()
                .iter()
                .map(|x| x.to_entity())
                .filter(|tx_hash| !tx_filter.contains(&tx_hash))
                .collect()
        };

        let transit_hashes: Vec<Byte32> = {
            let tx_pool = self.relayer.shared.shared().tx_pool_controller();
            let proposals: HashMap<packed::ProposalShortId, Byte32> = hashes
                .into_iter()
                .map(|tx_hash| (packed::ProposalShortId::from_tx_hash(&tx_hash), tx_hash))
                .collect();
            // TODO: error handle
            let fresh_ids = tx_pool
                .fresh_proposals_filter(proposals.keys().cloned().collect())
                .unwrap()
                .wait()
                .unwrap();
            fresh_ids
                .iter()
                .filter_map(|id| proposals.get(id))
                .cloned()
                .collect()
        };

        if transit_hashes.is_empty() {
            return Ok(());
        }

        if let Some(peer_state) = self
            .relayer
            .shared()
            .peers()
            .state
            .write()
            .get_mut(&self.peer)
        {
            let mut inflight_transactions = self.relayer.shared().inflight_transactions();

            debug_target!(
                crate::LOG_TARGET_RELAY,
                "transaction({:?}) from {} not known, get it from the peer",
                &transit_hashes,
                self.peer,
            );

            for tx_hash in transit_hashes {
                let last_ask_timeout = inflight_transactions.get(&tx_hash).cloned();

                if let Some(next_ask_timeout) =
                    peer_state.add_ask_for_tx(tx_hash.clone(), last_ask_timeout)
                {
                    inflight_transactions.insert(tx_hash, next_ask_timeout);
                }
            }
        }

        Ok(())
    }
}
