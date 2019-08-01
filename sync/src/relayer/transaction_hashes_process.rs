use crate::relayer::compact_block::TransactionHashes;
use crate::relayer::Relayer;
use ckb_core::transaction::ProposalShortId;
use ckb_logger::{debug_target, trace_target};
use ckb_network::PeerIndex;
use ckb_protocol::RelayTransactionHashes as FbsRelayTransactionHashes;
use ckb_store::ChainStore;
use failure::Error as FailureError;
use numext_fixed_hash::H256;
use std::convert::TryInto;

pub struct TransactionHashesProcess<'a, CS> {
    message: &'a FbsRelayTransactionHashes<'a>,
    relayer: &'a Relayer<CS>,
    peer: PeerIndex,
}

impl<'a, CS: ChainStore + 'static> TransactionHashesProcess<'a, CS> {
    pub fn new(
        message: &'a FbsRelayTransactionHashes,
        relayer: &'a Relayer<CS>,
        peer: PeerIndex,
    ) -> Self {
        TransactionHashesProcess {
            message,
            relayer,
            peer,
        }
    }

    pub fn execute(self) -> Result<(), FailureError> {
        let transaction_hashes: TransactionHashes = (*self.message).try_into()?;

        let mut transit_hashes: Vec<H256> = {
            let tx_filter = self.relayer.shared().tx_filter();
            transaction_hashes
                .hashes
                .into_iter()
                .filter(|tx_hash| {
                    if tx_filter.contains(&tx_hash) {
                        debug_target!(
                            crate::LOG_TARGET_RELAY,
                            "transaction({:#x}) from {} already known, ignore it",
                            tx_hash,
                            self.peer,
                        );
                        false
                    } else {
                        true
                    }
                })
                .collect()
        };

        let mut knowned = Vec::with_capacity(transit_hashes.len());
        {
            let state = self.relayer.shared.lock_chain_state();
            let tx_pool = state.tx_pool();

            transit_hashes.retain(|tx_hash| {
                let short_id = ProposalShortId::from_tx_hash(&tx_hash);
                if tx_pool.contains_tx(&short_id) {
                    trace_target!(
                        crate::LOG_TARGET_RELAY,
                        "transaction({:#x}) from {} already in transaction pool, ignore it",
                        tx_hash,
                        self.peer,
                    );
                    knowned.push(tx_hash.to_owned());
                    false
                } else {
                    true
                }
            })
        }

        {
            self.relayer.shared().mark_as_known_txs(knowned);
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

            for tx_hash in transit_hashes {
                debug_target!(
                    crate::LOG_TARGET_RELAY,
                    "transaction({:#x}) from {} not known, get it from the peer",
                    tx_hash,
                    self.peer,
                );

                let last_ask_timeout = inflight_transactions.get(&tx_hash).cloned();

                if let Some(next_ask_timeout) =
                    peer_state.add_ask_for_tx(tx_hash.clone(), last_ask_timeout)
                {
                    inflight_transactions.insert(tx_hash.clone(), next_ask_timeout);
                }
            }
        }

        Ok(())
    }
}
