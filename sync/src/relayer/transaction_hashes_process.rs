use crate::relayer::{Relayer, MAX_RELAY_TXS_NUM_PER_BATCH};
use ckb_logger::{debug_target, warn};
use ckb_network::PeerIndex;
use ckb_types::{
    packed::{self, Byte32},
    prelude::*,
};
use ckb_util::LinkedHashMap;
use failure::{err_msg, Error as FailureError};

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
        let state = self.relayer.shared().state();
        {
            let relay_transaction_hashes = self.message;
            if relay_transaction_hashes.tx_hashes().len() > MAX_RELAY_TXS_NUM_PER_BATCH {
                warn!("Peer {} sends us an invalid message, RelayTransactionHashes tx_hashes size ({}) is greater than MAX_RELAY_TXS_NUM_PER_BATCH ({})",
                    self.peer, relay_transaction_hashes.tx_hashes().len(), MAX_RELAY_TXS_NUM_PER_BATCH);
                return Err(err_msg(
                    "RelayTransactionHashes tx_hashes size is greater than MAX_RELAY_TXS_NUM_PER_BATCH"
                        .to_owned(),
                ));
            }
        }

        let hashes: Vec<Byte32> = {
            let tx_filter = state.tx_filter();
            self.message
                .tx_hashes()
                .iter()
                .map(|x| x.to_entity())
                .filter(|tx_hash| !tx_filter.contains(&tx_hash))
                .collect()
        };

        let transit_hashes: Vec<Byte32> = {
            let tx_pool = self.relayer.shared.shared().tx_pool_controller();
            let mut proposals: LinkedHashMap<packed::ProposalShortId, Byte32> = hashes
                .into_iter()
                .map(|tx_hash| (packed::ProposalShortId::from_tx_hash(&tx_hash), tx_hash))
                .collect();
            let fresh_ids = tx_pool
                .fresh_proposals_filter(proposals.keys().cloned().collect())
                .map_err(|e| {
                    debug_target!(
                        crate::LOG_TARGET_RELAY,
                        "[TransactionHashesProcess] request fresh_proposals_filter error {:?}",
                        e
                    );
                    e
                })?;
            fresh_ids
                .into_iter()
                .filter_map(|id| proposals.remove(&id))
                .collect()
        };

        if transit_hashes.is_empty() {
            return Ok(());
        }

        if let Some(peer_state) = state.peers().state.write().get_mut(&self.peer) {
            let mut inflight_transactions = state.inflight_transactions();

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
