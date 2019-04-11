use crate::relayer::Relayer;
use crate::relayer::MAX_RELAY_PEERS;
use ckb_core::{transaction::Transaction, Cycle};
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_protocol::{RelayMessage, ValidTransaction as FbsValidTransaction};
use ckb_shared::index::ChainIndex;
use ckb_shared::tx_pool::types::PoolError;
use ckb_traits::chain_provider::ChainProvider;
use ckb_util::TryInto;
use ckb_verification::TransactionError;
use failure::Error as FailureError;
use flatbuffers::FlatBufferBuilder;
use log::{debug, warn};
use numext_fixed_hash::H256;
use std::time::Duration;

const DEFAULT_BAN_TIME: Duration = Duration::from_secs(3600 * 24 * 3);

pub struct TransactionProcess<'a, CI: ChainIndex + 'a> {
    message: &'a FbsValidTransaction<'a>,
    relayer: &'a Relayer<CI>,
    peer: PeerIndex,
    nc: &'a mut CKBProtocolContext,
}

impl<'a, CI> TransactionProcess<'a, CI>
where
    CI: ChainIndex + 'static,
{
    pub fn new(
        message: &'a FbsValidTransaction,
        relayer: &'a Relayer<CI>,
        peer: PeerIndex,
        nc: &'a mut CKBProtocolContext,
    ) -> Self {
        TransactionProcess {
            message,
            nc,
            relayer,
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
            let max_block_cycles = self.relayer.shared.consensus().max_block_cycles();
            chain_state.add_tx_to_pool(tx.clone(), max_block_cycles)
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
                    .filter(|peer_index| {
                        known_txs.insert(*peer_index, tx_hash.clone()) && (self.peer != *peer_index)
                    })
                    .take(MAX_RELAY_PEERS)
                    .collect();

                for peer in selected_peers {
                    let ret = self.nc.send(peer, fbb.finished_data().to_vec());
                    if ret.is_err() {
                        warn!(target: "relay", "relay Transaction error {:?}", ret);
                    }
                }
            }
            Err(PoolError::InvalidTx(TransactionError::UnknownInput))
            | Err(PoolError::InvalidTx(TransactionError::Conflict)) => {
                // this error may occured when peer's tip is different with us,
                // we can't proof peer is bad so just ignore this
                debug!(target: "relay", "peer {} relay a conflict or missing input tx: {:?}", self.peer, tx);
            }
            Ok(cycles) => {
                debug!(target: "relay", "peer {} relay wrong cycles tx: {:?} real cycles {} wrong cycles {}", self.peer, tx, cycles, relay_cycles);
                // TODO use report score interface
                self.nc.ban_peer(self.peer, DEFAULT_BAN_TIME);
            }
            Err(err) => {
                debug!(target: "relay", "peer {} relay a invalid tx: {:?}, error: {:?}", self.peer, tx, err);
                // TODO use report score interface
                self.nc.ban_peer(self.peer, DEFAULT_BAN_TIME);
            }
        }

        Ok(())
    }

    fn already_known(&self, hash: H256) -> bool {
        let mut tx_filter = self.relayer.state.tx_filter.lock();
        tx_filter.insert(hash, ()).is_some()
    }
}
