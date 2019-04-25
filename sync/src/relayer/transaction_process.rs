use crate::relayer::Relayer;
use crate::relayer::MAX_RELAY_PEERS;
use ckb_core::{transaction::Transaction, Cycle};
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_protocol::{RelayMessage, RelayTransaction as FbsRelayTransaction};
use ckb_shared::store::ChainStore;
use ckb_shared::tx_pool::types::PoolError;
use ckb_verification::TransactionError;
use failure::Error as FailureError;
use flatbuffers::FlatBufferBuilder;
use log::debug;
use numext_fixed_hash::H256;
use std::convert::TryInto;
use std::time::Duration;

const DEFAULT_BAN_TIME: Duration = Duration::from_secs(3600 * 24 * 3);

pub struct TransactionProcess<'a, CS> {
    message: &'a FbsRelayTransaction<'a>,
    relayer: &'a Relayer<CS>,
    nc: &'a CKBProtocolContext,
    peer: PeerIndex,
}

impl<'a, CS: ChainStore> TransactionProcess<'a, CS> {
    pub fn new(
        message: &'a FbsRelayTransaction,
        relayer: &'a Relayer<CS>,
        nc: &'a CKBProtocolContext,
        peer: PeerIndex,
    ) -> Self {
        TransactionProcess {
            message,
            relayer,
            nc,
            peer,
        }
    }

    pub fn execute(self) -> Result<(), FailureError> {
        let (tx, relay_cycles): (Transaction, Cycle) = (*self.message).try_into()?;
        let tx_hash = tx.hash();

        if self.already_known(tx_hash.clone()) {
            debug!(target: "relay", "discarding already known transaction {:#x}", tx_hash);
            return Ok(());
        }

        let tx_result = {
            let chain_state = self.relayer.shared.chain_state().lock();
            chain_state.add_tx_to_pool(tx.clone())
        };
        // disconnect peer if cycles mismatch
        match tx_result {
            Ok(cycles) if cycles == relay_cycles => {
                // broadcast tx
                let fbb = &mut FlatBufferBuilder::new();
                let message = RelayMessage::build_transaction(fbb, &tx, cycles);
                fbb.finish(message, None);

                let mut known_txs = self.relayer.peers.known_txs.lock();
                let selected_peers: Vec<PeerIndex> = self
                    .nc
                    .connected_peers()
                    .into_iter()
                    .filter(|target_peer| {
                        known_txs.insert(*target_peer, tx_hash.clone())
                            && (self.peer != *target_peer)
                    })
                    .take(MAX_RELAY_PEERS)
                    .collect();

                for target_peer in selected_peers {
                    self.nc
                        .send_message_to(target_peer, fbb.finished_data().to_vec());
                }
            }
            Err(PoolError::InvalidTx(TransactionError::NullInput))
            | Err(PoolError::InvalidTx(TransactionError::NullDep))
            | Err(PoolError::InvalidTx(TransactionError::CapacityOverflow))
            | Err(PoolError::InvalidTx(TransactionError::DuplicateInputs))
            | Err(PoolError::InvalidTx(TransactionError::Empty))
            | Err(PoolError::InvalidTx(TransactionError::OutputsSumOverflow))
            | Err(PoolError::InvalidTx(TransactionError::InvalidScript))
            | Err(PoolError::InvalidTx(TransactionError::ScriptFailure(_)))
            | Err(PoolError::InvalidTx(TransactionError::InvalidSignature))
            | Err(PoolError::InvalidTx(TransactionError::InvalidValidSince)) => {
                debug!(target: "relay", "peer {} relay a invalid tx: {:?}", self.peer, tx);
                // TODO use report score interface
                self.nc.ban_peer(self.peer, DEFAULT_BAN_TIME);
            }
            Ok(cycles) => {
                debug!(
                    target: "relay",
                    "peer {} relay wrong cycles tx: {:?} real cycles {} wrong cycles {}",
                    self.peer, tx, cycles, relay_cycles,
                );
                // TODO use report score interface
                self.nc.ban_peer(self.peer, DEFAULT_BAN_TIME);
            }
            Err(err) => {
                // this error may occured when peer's tip is different with us,
                // we can't proof peer is bad so just ignore this
                debug!(target: "relay", "peer {} relay tx verify err: {:?}, error: {:?}", self.peer, tx, err);
            }
        }

        Ok(())
    }

    fn already_known(&self, hash: H256) -> bool {
        let mut tx_filter = self.relayer.state.tx_filter.lock();
        tx_filter.insert(hash, ()).is_some()
    }
}
