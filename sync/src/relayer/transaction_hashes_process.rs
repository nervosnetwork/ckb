use crate::relayer::compact_block::TransactionHashes;
use crate::relayer::Relayer;
use ckb_core::transaction::ProposalShortId;
use ckb_logger::debug_target;
use ckb_network::PeerIndex;
use ckb_protocol::RelayTransactionHashes as FbsRelayTransactionHashes;
use failure::Error as FailureError;
use numext_fixed_hash::H256;
use std::convert::TryInto;

pub struct TransactionHashesProcess<'a> {
    message: &'a FbsRelayTransactionHashes<'a>,
    relayer: &'a Relayer,
    peer: PeerIndex,
}

impl<'a> TransactionHashesProcess<'a> {
    pub fn new(
        message: &'a FbsRelayTransactionHashes,
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
        let transaction_hashes: TransactionHashes = (*self.message).try_into()?;

        let mut transit_hashes: Vec<H256> = {
            let tx_filter = self.relayer.shared().tx_filter();
            transaction_hashes
                .hashes
                .into_iter()
                .filter(|tx_hash| !tx_filter.contains(&tx_hash))
                .collect()
        };

        {
            let state = self.relayer.shared.lock_chain_state();
            let tx_pool = state.tx_pool();

            transit_hashes
                .retain(|tx_hash| !tx_pool.contains_tx(&ProposalShortId::from_tx_hash(&tx_hash)))
        }

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
                    inflight_transactions.insert(tx_hash, next_ask_timeout);
                }
            }
        }

        Ok(())
    }
}
