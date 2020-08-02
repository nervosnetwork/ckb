use crate::relayer::Relayer;
use crate::{Status, StatusCode};
use ckb_error::{Error, ErrorKind, InternalError, InternalErrorKind};
use ckb_logger::debug_target;
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_types::{
    core::{error::OutPointError, Cycle, TransactionView},
    packed,
    prelude::*,
};
use ckb_util::LinkedHashSet;
use ckb_verification::cache::CacheEntry;
use ckb_verification::TransactionError;
use sentry::{capture_message, with_scope, Level};
use std::collections::VecDeque;
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

    pub fn execute(self) -> Status {
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
            return Status::ok();
        }

        // Insert tx_hash into `already_known`
        // Remove tx_hash from `inflight_transactions`
        {
            shared_state.mark_as_known_txs(txs.iter().map(|(tx, _)| tx.hash()));
        }

        // Remove tx_hash from `tx_ask_for_set`
        {
            if let Some(peer_state) = shared_state.peers().state.write().get_mut(&self.peer) {
                for (tx, _) in txs.iter() {
                    peer_state.remove_ask_for_tx(&tx.hash());
                }
            }
        }

        let tx_pool = self.relayer.shared.shared().tx_pool_controller();
        for (tx, declared_cycle) in txs {
            if declared_cycle > self.relayer.max_tx_verify_cycles {
                debug_target!(
                    crate::LOG_TARGET_RELAY,
                    "ignore tx {} which declared cycles({}) is large than max tx verify cycles {}",
                    tx.hash(),
                    declared_cycle,
                    self.relayer.max_tx_verify_cycles
                );
                continue;
            }

            match tx_pool.submit_tx(tx.clone()) {
                Ok(ret) => {
                    if self.handle_submit_result(ret, declared_cycle, tx).is_err() {
                        break;
                    }
                }
                Err(err) => {
                    return StatusCode::TxPool
                        .with_context(format!("TxPool submit_tx error: {:?}", err));
                }
            };
        }

        Status::ok()
    }

    fn process_orphan_tx(&self, hash: packed::Byte32) {
        let tx_pool = self.relayer.shared.shared().tx_pool_controller();
        let mut orphan: VecDeque<packed::Byte32> = VecDeque::new();
        orphan.push_back(hash);

        while let Some(tx_hash) = orphan.pop_front() {
            if let Some(entry) = self.relayer.orphan_tx_pool.get(&tx_hash) {
                let tx = entry.tx;
                match tx_pool.submit_tx(tx.clone()) {
                    Ok(ret) => match ret {
                        Ok(_) => {
                            self.relayer.orphan_tx_pool.remove_orphan_tx(&tx_hash);
                            self.broadcast_tx(tx_hash);
                            if let Some(hash) = self.relayer.orphan_tx_pool.find_by_previous(&tx) {
                                orphan.push_back(hash);
                            }
                        }
                        Err(err) => {
                            if !is_missing_input(&err) {
                                self.relayer.orphan_tx_pool.remove_orphan_tx(&tx_hash);
                            }
                            if is_malformed(&err) {
                                self.ban_malformed(&err);
                                break;
                            }
                        }
                    },
                    Err(err) => {
                        debug_target!(
                            crate::LOG_TARGET_RELAY,
                            "process_orphan_tx internal error {}",
                            err
                        );
                        break;
                    }
                }
            }
        }
    }

    fn broadcast_tx(&self, tx_hash: packed::Byte32) {
        let mut map = self.relayer.shared().state().tx_hashes();
        let set = map.entry(self.peer).or_insert_with(LinkedHashSet::default);
        set.insert(tx_hash);
    }

    fn handle_submit_result(
        &self,
        ret: Result<CacheEntry, Error>,
        declared_cycle: Cycle,
        tx: TransactionView,
    ) -> Result<(), ()> {
        let tx_hash = tx.hash();
        match ret {
            Ok(verified) => {
                if declared_cycle == verified.cycles {
                    self.broadcast_tx(tx_hash);

                    // Recursively process orphan transactions that depended on this one
                    if let Some(hash) = self.relayer.orphan_tx_pool.find_by_previous(&tx) {
                        self.process_orphan_tx(hash)
                    }
                    Ok(())
                } else {
                    debug_target!(
                        crate::LOG_TARGET_RELAY,
                        "peer {} relay wrong cycles tx_hash: {} verified cycles {} declared cycles {}",
                        self.peer,
                        tx_hash,
                        verified.cycles,
                        declared_cycle,
                    );

                    self.nc.ban_peer(
                        self.peer,
                        DEFAULT_BAN_TIME,
                        String::from("send us a transaction with wrong cycles"),
                    );

                    Err(())
                }
            }
            Err(err) => self.handle_submit_error(&err, tx),
        }
    }

    fn ban_malformed(&self, error: &Error) {
        with_scope(
            |scope| scope.set_fingerprint(Some(&["ckb-sync", "relay-invalid-tx"])),
            || {
                capture_message(
                    &format!(
                        "Ban peer {} for {} seconds, reason: \
                         relay invalid tx, error: {:?}",
                        self.peer,
                        DEFAULT_BAN_TIME.as_secs(),
                        error
                    ),
                    Level::Info,
                )
            },
        );
        self.nc.ban_peer(
            self.peer,
            DEFAULT_BAN_TIME,
            String::from("send us an invalid transaction"),
        );
    }

    fn handle_submit_error(&self, error: &Error, tx: TransactionView) -> Result<(), ()> {
        if is_missing_input(error) {
            self.relayer.orphan_tx_pool.add_orphan_tx(tx, self.peer);
        } else if is_malformed(error) {
            self.ban_malformed(error);
            return Err(());
        }
        Ok(())
    }
}

fn is_missing_input(error: &Error) -> bool {
    if error.kind() == &ErrorKind::OutPoint {
        error
            .downcast_ref::<OutPointError>()
            .expect("error kind checked")
            .is_unknown()
    } else {
        false
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
