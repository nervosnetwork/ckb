use crate::relayer::Relayer;
use ckb_logger::debug_target;
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_tx_pool::error::PoolError;
use ckb_types::{
    core::{Cycle, TransactionView},
    packed,
    prelude::*,
};
use failure::Error as FailureError;
use sentry::{capture_message, with_scope, Level};
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

        let txs: Vec<(TransactionView, Cycle)> = {
            let tx_filter = self.relayer.shared().tx_filter();

            relay_txs
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
            self.relayer
                .shared
                .mark_as_known_txs(txs.iter().map(|(tx, _)| tx.hash()).collect());
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
                for (tx, _) in txs.iter() {
                    peer_state.remove_ask_for_tx(&tx.hash());
                }
            }
        }

        let mut tx_views = Vec::with_capacity(txs.len());

        let relay_cycles_vec: Vec<_> = txs
            .into_iter()
            .map(|(tx, relay_cycles)| {
                let tx_hash = tx.hash();
                tx_views.push(tx);
                (tx_hash, relay_cycles)
            })
            .collect();
        let nc = Arc::clone(&self.nc);
        let peer_index = self.peer;
        let shared = Arc::clone(self.relayer.shared());

        let callback = Box::new(move |ret: Result<Vec<Cycle>, PoolError>| match ret {
            Ok(cycles_vec) => {
                for ((tx_hash, relay_cycles), cycles) in
                    relay_cycles_vec.into_iter().zip(cycles_vec.into_iter())
                {
                    if relay_cycles == cycles {
                        let mut cache = shared.tx_hashes();
                        let entry = cache.entry(peer_index).or_insert_with(HashSet::default);
                        entry.insert(tx_hash);
                    } else {
                        debug_target!(
                            crate::LOG_TARGET_RELAY,
                            "peer {} relay wrong cycles tx_hash: {} real cycles {} wrong cycles {}",
                            peer_index,
                            tx_hash,
                            cycles,
                            relay_cycles,
                        );

                        nc.ban_peer(peer_index, DEFAULT_BAN_TIME);
                        break;
                    }
                }
            }
            Err(err) => {
                if err.is_bad_tx() {
                    debug_target!(
                        crate::LOG_TARGET_RELAY,
                        "peer {} relay a invalid tx, error: {:?}",
                        peer_index,
                        err
                    );
                    with_scope(
                        |scope| scope.set_fingerprint(Some(&["ckb-sync", "relay-invalid-tx"])),
                        || {
                            capture_message(
                                &format!(
                                    "ban peer {} {:?}, reason: \
                                     relay invalid tx, error: {:?}",
                                    peer_index, DEFAULT_BAN_TIME, err
                                ),
                                Level::Info,
                            )
                        },
                    );
                    nc.ban_peer(peer_index, DEFAULT_BAN_TIME);
                } else {
                    debug_target!(
                        crate::LOG_TARGET_RELAY,
                        "peer {} relay a conflict or missing input, error: {:?}",
                        peer_index,
                        err
                    );
                }
            }
        });

        let tx_pool = self.relayer.shared.shared().tx_pool_controller();
        if let Err(err) = tx_pool.notify_txs(tx_views, Some(callback)) {
            ckb_logger::debug!("relayer send future task error: {:?}", err);
        }

        Ok(())
    }
}
