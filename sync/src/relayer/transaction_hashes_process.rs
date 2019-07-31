use crate::relayer::compact_block::TransactionHashes;
use crate::relayer::Relayer;
use ckb_core::transaction::ProposalShortId;
use ckb_logger::{debug_target, trace_target};
use ckb_network::PeerIndex;
use ckb_protocol::RelayTransactionHashes as FbsRelayTransactionHashes;
use ckb_store::ChainStore;
use failure::Error as FailureError;
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

        for tx_hash in transaction_hashes.hashes {
            let short_id = ProposalShortId::from_tx_hash(&tx_hash);
            if self.relayer.shared().already_known_tx(&tx_hash) {
                debug_target!(
                    crate::LOG_TARGET_RELAY,
                    "transaction({:#x}) from {} already known, ignore it",
                    tx_hash,
                    self.peer,
                );
            } else if self
                .relayer
                .shared
                .lock_chain_state()
                .tx_pool()
                .get_tx_with_cycles(&short_id)
                .is_some()
            {
                trace_target!(
                    crate::LOG_TARGET_RELAY,
                    "transaction({:#x}) from {} already in transaction pool, ignore it",
                    tx_hash,
                    self.peer,
                );
                self.relayer.shared().mark_as_known_tx(tx_hash.clone());
            } else {
                debug_target!(
                    crate::LOG_TARGET_RELAY,
                    "transaction({:#x}) from {} not known, get it from the peer",
                    tx_hash,
                    self.peer,
                );
                let last_ask_timeout = self
                    .relayer
                    .shared()
                    .inflight_transactions()
                    .get(&tx_hash)
                    .cloned();
                if let Some(next_ask_timeout) = self
                    .relayer
                    .shared()
                    .peers()
                    .state
                    .write()
                    .get_mut(&self.peer)
                    .and_then(|peer_state| {
                        peer_state.add_ask_for_tx(tx_hash.clone(), last_ask_timeout)
                    })
                {
                    self.relayer
                        .shared()
                        .inflight_transactions()
                        .insert(tx_hash.clone(), next_ask_timeout);
                }
            }
        }
        Ok(())
    }
}
