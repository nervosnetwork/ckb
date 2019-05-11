use crate::relayer::Relayer;
use crate::relayer::MAX_RELAY_PEERS;
use ckb_core::{transaction::Transaction, Cycle};
use ckb_network::{CKBProtocolContext, PeerIndex, TargetSession};
use ckb_protocol::{RelayMessage, RelayTransaction as FbsRelayTransaction};
use ckb_store::ChainStore;
use failure::Error as FailureError;
use flatbuffers::FlatBufferBuilder;
use log::debug;
use std::convert::TryInto;
use std::sync::Arc;
use std::time::Duration;

const DEFAULT_BAN_TIME: Duration = Duration::from_secs(3600 * 24 * 3);

pub struct TransactionProcess<'a, CS> {
    message: &'a FbsRelayTransaction<'a>,
    relayer: &'a Relayer<CS>,
    nc: Arc<dyn CKBProtocolContext + Sync>,
    peer: PeerIndex,
}

impl<'a, CS: ChainStore + Sync + 'static> TransactionProcess<'a, CS> {
    pub fn new(
        message: &'a FbsRelayTransaction,
        relayer: &'a Relayer<CS>,
        nc: Arc<CKBProtocolContext + Sync>,
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

        if self.relayer.state.already_known(&tx_hash) {
            debug!(target: "relay", "discarding already known transaction {:#x}", tx_hash);
            return Ok(());
        }

        // Insert tx_hash into `already_known`
        // Remove tx_hash from `tx_already_asked`
        self.relayer.state.insert_tx(tx_hash.clone());
        // Remove tx_hash from `tx_ask_for_set`
        if let Some(peer_state) = self.relayer.peers.state.write().get_mut(&self.peer) {
            peer_state.remove_ask_for_tx(&tx_hash);
        }

        rayon::spawn({
            // prepare sync data
            let nc = Arc::clone(&self.nc);
            let self_peer = self.peer;
            let tx_pool_executor = Arc::clone(&self.relayer.tx_pool_executor);
            let peers = Arc::clone(&self.relayer.peers);
            let tx_hash = tx_hash.clone();
            let tx = tx.to_owned();
            move || {
                let tx_pool_executor = Arc::clone(&tx_pool_executor);
                let tx_result = tx_pool_executor.verify_and_add_tx_to_pool(tx.to_owned());
                // disconnect peer if cycles mismatch
                match tx_result {
                    Ok(cycles) if cycles == relay_cycles => {
                        let selected_peers: Vec<PeerIndex> = {
                            let mut known_txs = peers.known_txs.lock();
                            nc.connected_peers()
                                .into_iter()
                                .filter(|target_peer| {
                                    known_txs.insert(*target_peer, tx_hash.clone())
                                        && (self_peer != *target_peer)
                                })
                                .take(MAX_RELAY_PEERS)
                                .collect()
                        };

                        let fbb = &mut FlatBufferBuilder::new();
                        let message = RelayMessage::build_transaction_hash(fbb, &tx_hash);
                        fbb.finish(message, None);
                        let data = fbb.finished_data().into();
                        nc.filter_broadcast(TargetSession::Multi(selected_peers), data);
                    }
                    Ok(cycles) => {
                        debug!(
                        target: "relay",
                        "peer {} relay wrong cycles tx: {:?} real cycles {} wrong cycles {}",
                        self_peer, tx, cycles, relay_cycles,
                        );
                        nc.ban_peer(self_peer, DEFAULT_BAN_TIME);
                    }
                    Err(err) => {
                        if err.is_bad_tx() {
                            debug!(target: "relay", "peer {} relay a invalid tx: {:x}, error: {:?}", self_peer, tx_hash, err);
                            sentry::capture_message(
                                &format!(
                                    "ban peer {} {:?}, reason: relay invalid tx: {:?}, error: {:?}",
                                    self_peer, DEFAULT_BAN_TIME, tx, err
                                ),
                                sentry::Level::Info,
                            );
                            nc.ban_peer(self_peer, DEFAULT_BAN_TIME);
                        } else {
                            debug!(target: "relay", "peer {} relay a conflict or missing input tx: {:x}, error: {:?}", self_peer, tx_hash, err);
                        }
                    }
                }
            }
        });

        Ok(())
    }
}
