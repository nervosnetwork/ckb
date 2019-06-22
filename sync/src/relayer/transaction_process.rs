use crate::relayer::Relayer;
use crate::relayer::MAX_RELAY_PEERS;
use ckb_core::{transaction::Transaction, Cycle};
use ckb_logger::debug_target;
use ckb_network::{CKBProtocolContext, PeerIndex, TargetSession};
use ckb_protocol::{RelayMessage, RelayTransaction as FbsRelayTransaction};
use ckb_store::ChainStore;
use failure::Error as FailureError;
use flatbuffers::FlatBufferBuilder;
use futures::{self, future::FutureResult, lazy};
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

        if self.relayer.shared().already_known_tx(&tx_hash) {
            debug_target!(
                crate::LOG_TARGET_RELAY,
                "discarding already known transaction {:#x}",
                tx_hash
            );
            return Ok(());
        }

        // Insert tx_hash into `already_known`
        // Remove tx_hash from `tx_already_asked`
        self.relayer.shared().mark_as_known_tx(tx_hash.clone());
        // Remove tx_hash from `tx_ask_for_set`
        if let Some(peer_state) = self
            .relayer
            .shared()
            .peers()
            .state
            .write()
            .get_mut(&self.peer)
        {
            peer_state.remove_ask_for_tx(&tx_hash);
        }

        let ret = self.nc.future_task({
            // prepare sync data
            let nc = Arc::clone(&self.nc);
            let self_peer = self.peer;
            let tx_pool_executor = Arc::clone(&self.relayer.tx_pool_executor);
            let shared = Arc::clone(self.relayer.shared());
            let tx_hash = tx_hash.clone();
            let tx = tx.to_owned();
            Box::new(lazy(move || -> FutureResult<(), ()> {
                let tx_pool_executor = Arc::clone(&tx_pool_executor);
                let tx_result = tx_pool_executor.verify_and_add_tx_to_pool(tx.to_owned());
                // disconnect peer if cycles mismatch
                match tx_result {
                    Ok(cycles) if cycles == relay_cycles => {
                        let selected_peers: Vec<PeerIndex> = {
                            let mut known_txs = shared.known_txs();
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
                        if let Err(err) =
                            nc.filter_broadcast(TargetSession::Multi(selected_peers), data)
                        {
                            debug_target!(
                                crate::LOG_TARGET_RELAY,
                                "relayer send TransactionHash error: {:?}",
                                err,
                            );
                        }
                    }
                    Ok(cycles) => {
                        debug_target!(
                            crate::LOG_TARGET_RELAY,
                            "peer {} relay wrong cycles tx: {:?} real cycles {} wrong cycles {}",
                            self_peer,
                            tx,
                            cycles,
                            relay_cycles,
                        );
                        nc.ban_peer(self_peer, DEFAULT_BAN_TIME);
                    }
                    Err(err) => {
                        if err.is_bad_tx() {
                            debug_target!(
                                crate::LOG_TARGET_RELAY,
                                "peer {} relay a invalid tx: {:x}, error: {:?}",
                                self_peer,
                                tx_hash,
                                err
                            );
                            use sentry::{capture_message, with_scope, Level};
                            with_scope(
                                |scope| {
                                    scope.set_fingerprint(Some(&["ckb-sync", "relay-invalid-tx"]))
                                },
                                || {
                                    capture_message(
                                        &format!(
                                            "ban peer {} {:?}, reason: \
                                             relay invalid tx: {:?}, error: {:?}",
                                            self_peer, DEFAULT_BAN_TIME, tx, err
                                        ),
                                        Level::Info,
                                    )
                                },
                            );
                            nc.ban_peer(self_peer, DEFAULT_BAN_TIME);
                        } else {
                            debug_target!(
                                crate::LOG_TARGET_RELAY,
                                "peer {} relay a conflict or missing input tx: {:x}, error: {:?}",
                                self_peer,
                                tx_hash,
                                err
                            );
                        }
                    }
                }
                futures::future::ok(())
            }))
        });
        if let Err(err) = ret {
            ckb_logger::debug!("relayer send future task error: {:?}", err);
        }
        Ok(())
    }
}
