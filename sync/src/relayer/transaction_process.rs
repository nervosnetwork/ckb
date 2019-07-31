use crate::relayer::Relayer;
use ckb_core::{transaction::Transaction, Cycle};
use ckb_logger::debug_target;
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_protocol::RelayTransaction as FbsRelayTransaction;
use ckb_store::ChainStore;
use failure::Error as FailureError;
use fnv::FnvHashSet;
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

        let ret = self.nc.future_task(
            {
                // prepare sync data
                let nc = Arc::clone(&self.nc);
                let peer_index = self.peer;
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
                            let mut cache = shared.tx_hashes();
                            let entry = cache.entry(peer_index).or_insert_with(FnvHashSet::default);
                            entry.insert(tx_hash);
                        }
                        Ok(cycles) => {
                            debug_target!(
                            crate::LOG_TARGET_RELAY,
                            "peer {} relay wrong cycles tx: {:?} real cycles {} wrong cycles {}",
                            peer_index,
                            tx,
                            cycles,
                            relay_cycles,
                        );
                            nc.ban_peer(peer_index, DEFAULT_BAN_TIME);
                        }
                        Err(err) => {
                            if err.is_bad_tx() {
                                debug_target!(
                                    crate::LOG_TARGET_RELAY,
                                    "peer {} relay a invalid tx: {:x}, error: {:?}",
                                    peer_index,
                                    tx_hash,
                                    err
                                );
                                use sentry::{capture_message, with_scope, Level};
                                with_scope(
                                    |scope| {
                                        scope.set_fingerprint(Some(&[
                                            "ckb-sync",
                                            "relay-invalid-tx",
                                        ]))
                                    },
                                    || {
                                        capture_message(
                                            &format!(
                                                "ban peer {} {:?}, reason: \
                                                 relay invalid tx: {:?}, error: {:?}",
                                                peer_index, DEFAULT_BAN_TIME, tx, err
                                            ),
                                            Level::Info,
                                        )
                                    },
                                );
                                nc.ban_peer(peer_index, DEFAULT_BAN_TIME);
                            } else {
                                debug_target!(
                                crate::LOG_TARGET_RELAY,
                                "peer {} relay a conflict or missing input tx: {:x}, error: {:?}",
                                peer_index,
                                tx_hash,
                                err
                            );
                            }
                        }
                    }
                    futures::future::ok(())
                }))
            },
            true,
        );
        if let Err(err) = ret {
            ckb_logger::debug!("relayer send future task error: {:?}", err);
        }
        Ok(())
    }
}
