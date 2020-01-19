use crate::relayer::Relayer;
use ckb_error::{Error, ErrorKind, InternalError, InternalErrorKind};
use ckb_logger::debug_target;
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_script::IllTransactionChecker;
use ckb_types::{
    core::{Cycle, TransactionView},
    packed,
    prelude::*,
};
use ckb_util::LinkedHashSet;
use ckb_verification::cache::CacheEntry;
use ckb_verification::TransactionError;
use failure::Error as FailureError;
use sentry::{capture_message, with_scope, Level};
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
        nc: Arc<dyn CKBProtocolContext + Sync>,
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
        let shared_state = self.relayer.shared().state();
        let txs: Vec<(TransactionView, Cycle)> = {
            let tx_filter = shared_state.tx_filter();

            self.message
                .transactions()
                .iter()
                .map(|tx| {
                    (
                        tx.transaction().to_entity().into_view(),
                        tx.cycles().unpack(),
                    )
                })
                .filter(|(tx, _)| !tx_filter.contains(&tx.hash()))
                .collect()
        };

        if txs.is_empty() {
            return Ok(());
        }

        // Insert tx_hash into `already_known`
        // Remove tx_hash from `inflight_transactions`
        {
            shared_state.mark_as_known_txs(txs.iter().map(|(tx, _)| tx.hash()).collect());
        }

        // Remove tx_hash from `tx_ask_for_set`
        {
            if let Some(peer_state) = shared_state.peers().state.write().get_mut(&self.peer) {
                for (tx, _) in txs.iter() {
                    peer_state.remove_ask_for_tx(&tx.hash());
                }
            }
        }

        let mut notify_txs = Vec::with_capacity(txs.len());
        let max_tx_verify_cycles = self.relayer.max_tx_verify_cycles;
        let reject_known_bugs = self.relayer.reject_known_bugs;
        let relay_cycles_vec: Vec<_> = txs
            .into_iter()
            .filter_map(|(tx, relay_cycles)| {
                // skip txs which consume too much cycles
                if relay_cycles > max_tx_verify_cycles {
                    debug_target!(
                        crate::LOG_TARGET_RELAY,
                        "ignore tx {} which relay cycles({}) is large than max tx verify cycles {}",
                        tx.hash(),
                        relay_cycles,
                        max_tx_verify_cycles
                    );
                    return None;
                }
                // skip txs with known bugs
                if reject_known_bugs {
                    if let Err(e) = IllTransactionChecker::new(&tx).check() {
                        debug_target!(
                            crate::LOG_TARGET_RELAY,
                            "ignore tx {} with known bug: {:}",
                            tx.hash(),
                            e,
                        );
                        return None;
                    }
                }
                let tx_hash = tx.hash();
                let tx_size = tx.data().serialized_size_in_block();
                notify_txs.push(tx);
                Some((tx_hash, relay_cycles, tx_size))
            })
            .collect();
        if notify_txs.is_empty() {
            return Ok(());
        }
        let nc = Arc::clone(&self.nc);
        let peer_index = self.peer;
        let shared = Arc::clone(self.relayer.shared());
        let min_fee_rate = self.relayer.min_fee_rate;

        let callback = Box::new(move |ret: Result<Vec<CacheEntry>, Error>| match ret {
            Ok(cache_entry_vec) => {
                for ((tx_hash, relay_cycles, tx_size), cache_entry) in relay_cycles_vec
                    .into_iter()
                    .zip(cache_entry_vec.into_iter())
                {
                    if relay_cycles == cache_entry.cycles {
                        if cache_entry.fee < min_fee_rate.fee(tx_size) {
                            debug_target!(
                                crate::LOG_TARGET_RELAY,
                                "peer {} relay tx lower than min fee rate {} shannons/KB. \
                                 tx: {:?}  size {} fee {}",
                                peer_index,
                                min_fee_rate,
                                tx_hash,
                                tx_size,
                                cache_entry.fee,
                            );
                            continue;
                        }
                        let mut cache = shared.state().tx_hashes();
                        let entry = cache
                            .entry(peer_index)
                            .or_insert_with(LinkedHashSet::default);
                        entry.insert(tx_hash);
                    } else {
                        debug_target!(
                            crate::LOG_TARGET_RELAY,
                            "peer {} relay wrong cycles tx_hash: {} real cycles {} wrong cycles {}",
                            peer_index,
                            tx_hash,
                            cache_entry.cycles,
                            relay_cycles,
                        );

                        nc.ban_peer(
                            peer_index,
                            DEFAULT_BAN_TIME,
                            String::from("send us a transaction with wrong cycles"),
                        );
                        break;
                    }
                }
            }
            Err(err) => {
                if is_malformed(&err) {
                    debug_target!(
                        crate::LOG_TARGET_RELAY,
                        "peer {} relay an invalid tx, error: {:?}",
                        peer_index,
                        err
                    );
                    with_scope(
                        |scope| scope.set_fingerprint(Some(&["ckb-sync", "relay-invalid-tx"])),
                        || {
                            capture_message(
                                &format!(
                                    "Ban peer {} for {} seconds, reason: \
                                     relay invalid tx, error: {:?}",
                                    peer_index,
                                    DEFAULT_BAN_TIME.as_secs(),
                                    err
                                ),
                                Level::Info,
                            )
                        },
                    );
                    nc.ban_peer(
                        peer_index,
                        DEFAULT_BAN_TIME,
                        String::from("send us an invalid transaction"),
                    );
                } else {
                    debug_target!(
                        crate::LOG_TARGET_RELAY,
                        "peer {} relay a conflict or missing input, error: {}",
                        peer_index,
                        err
                    );
                }
            }
        });

        let tx_pool = self.relayer.shared.shared().tx_pool_controller();
        if let Err(err) = tx_pool.notify_txs(notify_txs, Some(callback)) {
            ckb_logger::debug!("relayer send future task error: {:?}", err);
        }

        Ok(())
    }
}

fn is_malformed(error: &Error) -> bool {
    match error.kind() {
        ErrorKind::Transaction => error
            .downcast_ref::<TransactionError>()
            .expect("error kind checked")
            .is_malformed_tx(),
        ErrorKind::Script => true,
        ErrorKind::Internal => {
            error
                .downcast_ref::<InternalError>()
                .expect("error kind checked")
                .kind()
                == &InternalErrorKind::CapacityOverflow
        }
        _ => false,
    }
}
