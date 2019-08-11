use crate::relayer::Relayer;
use ckb_error::{Error, ErrorKind};
use ckb_logger::debug_target;
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_types::{
    core::Cycle,
    packed::{self, Byte32, RelayTransaction},
    prelude::*,
};
use ckb_verification::TransactionError;
use failure::Error as FailureError;
use futures::{self, future::FutureResult, lazy};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

const DEFAULT_BAN_TIME: Duration = Duration::from_secs(3600 * 24 * 3);

pub struct TransactionsProcess<'a> {
    message: packed::RelayTransactionsReader<'a>,
    relayer: &'a Relayer,
    nc: Arc<dyn CKBProtocolContext + Sync>,
    peer: PeerIndex,
}

impl<'a> TransactionsProcess<'a> {
    pub fn new(
        message: packed::RelayTransactionsReader<'a>,
        relayer: &'a Relayer,
        nc: Arc<CKBProtocolContext + Sync>,
        peer: PeerIndex,
    ) -> Self {
        TransactionsProcess {
            message,
            relayer,
            nc,
            peer,
        }
    }

    pub fn execute(self) -> Result<(), FailureError> {
        let relay_txs = self.message.transactions();

        let txs: Vec<(Byte32, RelayTransaction)> = {
            let tx_filter = self.relayer.shared().tx_filter();

            relay_txs
                .iter()
                .map(|tx| (tx.transaction().calc_tx_hash(), tx))
                .filter(|(hash, _)| !tx_filter.contains(&hash))
                .map(|(hash, tx)| (hash, tx.to_entity()))
                .collect()
        };

        if txs.is_empty() {
            return Ok(());
        }

        // Insert tx_hash into `already_known`
        // Remove tx_hash from `inflight_transactions`
        {
            self.relayer
                .shared
                .mark_as_known_txs(txs.iter().map(|(hash, _)| hash.clone()).collect());
        }

        // Remove tx_hash from `tx_ask_for_set`
        {
            if let Some(peer_state) = self
                .relayer
                .shared()
                .peers()
                .state
                .write()
                .get_mut(&self.peer)
            {
                for (hash, _) in txs.iter() {
                    peer_state.remove_ask_for_tx(hash);
                }
            }
        }

        let ret = self.nc.future_task(
            {
                // prepare sync data
                let nc = Arc::clone(&self.nc);
                let peer_index = self.peer;
                let shared = Arc::clone(self.relayer.shared());
                let tx_pool_executor = Arc::clone(&self.relayer.tx_pool_executor);
                Box::new(lazy(move || -> FutureResult<(), ()> {
                    for (_,relay_tx) in txs.into_iter() {
                        let relay_cycles: Cycle = relay_tx.cycles().unpack();
                        let tx = relay_tx.transaction().into_view();
                        let tx_hash = tx.hash();
                        let tx_result = tx_pool_executor.verify_and_add_tx_to_pool(tx.clone());
                        // disconnect peer if cycles mismatch
                        match tx_result {
                            Ok(cycles) if cycles == relay_cycles => {
                                let mut cache = shared.tx_hashes();
                                let entry = cache.entry(peer_index).or_insert_with(HashSet::default);
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
                                break;
                            }
                            Err(err) => {
                                if is_bad_tx(&err) {
                                    debug_target!(
                                        crate::LOG_TARGET_RELAY,
                                        "peer {} relay a invalid tx: {}, error: {:?}",
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
                                    break;
                                } else {
                                    debug_target!(
                                        crate::LOG_TARGET_RELAY,
                                        "peer {} relay a conflict or missing input tx: {}, error: {:?}",
                                        peer_index,
                                        tx_hash,
                                        err
                                    );
                                }
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

fn is_bad_tx(error: &Error) -> bool {
    match error.kind() {
        ErrorKind::Transaction => TryInto::<&TransactionError>::try_into(error)
            .expect("error kind checked")
            .is_bad_tx(),
        ErrorKind::Script => true,
        _ => false,
    }
}
