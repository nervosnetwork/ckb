use crate::relayer::compact_block::{RelayTransaction, RelayTransactions};
use crate::relayer::Relayer;
use ckb_logger::debug_target;
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_protocol::RelayTransactions as FbsRelayTransactions;
use failure::Error as FailureError;
use fnv::FnvHashSet;
use futures::{self, future::FutureResult, lazy};
use std::convert::TryInto;
use std::sync::Arc;
use std::time::Duration;

const DEFAULT_BAN_TIME: Duration = Duration::from_secs(3600 * 24 * 3);

pub struct TransactionsProcess<'a> {
    message: &'a FbsRelayTransactions<'a>,
    relayer: &'a Relayer,
    nc: Arc<dyn CKBProtocolContext + Sync>,
    peer: PeerIndex,
}

impl<'a> TransactionsProcess<'a> {
    pub fn new(
        message: &'a FbsRelayTransactions,
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
        let relay_txs: RelayTransactions = (*self.message).try_into()?;

        let txs: Vec<RelayTransaction> = {
            let tx_filter = self.relayer.shared().tx_filter();

            relay_txs
                .transactions
                .into_iter()
                .filter(|relay_tx| !tx_filter.contains(relay_tx.transaction.hash()))
                .collect()
        };

        if txs.is_empty() {
            return Ok(());
        }

        // Insert tx_hash into `already_known`
        // Remove tx_hash from `inflight_transactions`
        {
            self.relayer.shared.mark_as_known_txs(
                txs.iter()
                    .map(|tx| tx.transaction.hash().to_owned())
                    .collect(),
            );
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
                for tx in txs.iter() {
                    peer_state.remove_ask_for_tx(tx.transaction.hash());
                }
            }
        }

        let ret = self.nc.future_task(
            {
                // prepare sync data
                let nc = Arc::clone(&self.nc);
                let peer_index = self.peer;
                let tx_pool_executor = Arc::clone(&self.relayer.tx_pool_executor);
                let shared = Arc::clone(self.relayer.shared());
                Box::new(lazy(move || -> FutureResult<(), ()> {
                    let tx_pool_executor = Arc::clone(&tx_pool_executor);

                    for relay_tx in txs.into_iter() {
                        let relay_cycles = relay_tx.cycles;
                        let tx = relay_tx.transaction;
                        let tx_hash = tx.hash().to_owned();
                        let tx_result = tx_pool_executor.verify_and_add_tx_to_pool(tx.clone());
                        // disconnect peer if cycles mismatch
                        match tx_result {
                            Ok((cycles, fee)) if cycles == relay_cycles => {
                                let min_fee_rate = shared.shared().min_fee_rate();
                                if fee < min_fee_rate.fee(tx.serialized_size()) {
                                    debug_target!(
                                        crate::LOG_TARGET_RELAY,
                                        "peer {} relay tx lower than our min fee rate {} shannons per bytes. tx: {:?}  size {} fee {}",
                                        peer_index,
                                        min_fee_rate,
                                        tx,
                                        tx.serialized_size(),
                                        fee,
                                    );
                                    break;
                                }
                                let mut cache = shared.tx_hashes();
                                let entry = cache.entry(peer_index).or_insert_with(FnvHashSet::default);
                                entry.insert(tx_hash);
                            }
                            Ok((cycles, _fee)) => {
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
                                    break;
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
