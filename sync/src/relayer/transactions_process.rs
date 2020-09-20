use crate::relayer::Relayer;
use crate::Status;
use ckb_error::{Error, ErrorKind, InternalError, InternalErrorKind};
use ckb_logger::{debug_target, error};
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_types::{
    core::{error::OutPointError, Cycle, TransactionView},
    packed,
    prelude::*,
};
use ckb_util::LinkedHashSet;
use ckb_verification::cache::CacheEntry;
use ckb_verification::TransactionError;
#[cfg(feature = "with_sentry")]
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

        let tx_pool = self.relayer.shared.shared().tx_pool_controller().clone();
        let relayer = self.relayer.clone();
        let nc = Arc::clone(&self.nc);
        let peer = self.peer;
        self.relayer.shared.shared().async_handle().spawn(
            async move {
                for (tx, declared_cycle) in txs {
                    if declared_cycle > relayer.max_tx_verify_cycles {
                        debug_target!(
                            crate::LOG_TARGET_RELAY,
                            "ignore tx {} which declared cycles({}) is large than max tx verify cycles {}",
                            tx.hash(),
                            declared_cycle,
                            relayer.max_tx_verify_cycles
                        );
                        continue;
                    }

                    match tx_pool.async_submit_tx(tx.clone()).await {
                        Ok(ret) => {
                            if handle_submit_result(nc.as_ref(), &relayer, ret, declared_cycle, tx, peer).await.is_err() {
                                break;
                            }
                        }
                        Err(err) => {
                            error!("TxPool submit_tx error: {:?}", err);
                        }
                    };
                }
            }
        );

        Status::ok()
    }
}

async fn handle_submit_result(
    nc: &(dyn CKBProtocolContext + Sync),
    relayer: &Relayer,
    ret: Result<CacheEntry, Error>,
    declared_cycle: Cycle,
    tx: TransactionView,
    peer: PeerIndex,
) -> Result<(), ()> {
    let tx_hash = tx.hash();
    match ret {
        Ok(verified) => {
            if declared_cycle == verified.cycles {
                broadcast_tx(relayer, tx_hash, peer);

                // Recursively process orphan transactions that depended on this one
                if let Some(hash) = relayer.get_orphan_tx_hash_by_previous(&tx) {
                    process_orphan_tx(nc, relayer, hash, peer).await
                }
                Ok(())
            } else {
                debug_target!(
                    crate::LOG_TARGET_RELAY,
                    "peer {} relay wrong cycles tx_hash: {} verified cycles {} declared cycles {}",
                    peer,
                    tx_hash,
                    verified.cycles,
                    declared_cycle,
                );

                nc.ban_peer(
                    peer,
                    DEFAULT_BAN_TIME,
                    String::from("send us a transaction with wrong cycles"),
                );

                Err(())
            }
        }
        Err(err) => handle_submit_error(nc, relayer, &err, tx, peer),
    }
}

fn handle_submit_error(
    nc: &(dyn CKBProtocolContext + Sync),
    relayer: &Relayer,
    error: &Error,
    tx: TransactionView,
    peer: PeerIndex,
) -> Result<(), ()> {
    error!(
        "received tx {} submit error: {} peer: {}",
        tx.hash(),
        error,
        peer
    );
    if is_missing_input(error) {
        relayer.add_orphan_tx(tx, peer);
    } else if is_malformed(error) {
        ban_malformed(nc, error, peer);
        return Err(());
    }
    Ok(())
}

async fn process_orphan_tx(
    nc: &(dyn CKBProtocolContext + Sync),
    relayer: &Relayer,
    hash: packed::Byte32,
    peer: PeerIndex,
) {
    let tx_pool = relayer.shared.shared().tx_pool_controller();
    let mut orphan: VecDeque<packed::Byte32> = VecDeque::new();
    orphan.push_back(hash);

    while let Some(tx_hash) = orphan.pop_front() {
        if let Some(tx) = relayer.get_orphan_tx(&tx_hash) {
            match tx_pool.async_submit_tx(tx.clone()).await {
                Ok(ret) => match ret {
                    Ok(_) => {
                        relayer.remove_orphan_tx(&tx_hash);
                        broadcast_tx(relayer, tx_hash, peer);
                        if let Some(hash) = relayer.get_orphan_tx_hash_by_previous(&tx) {
                            orphan.push_back(hash);
                        }
                    }
                    Err(err) => {
                        if !is_missing_input(&err) {
                            relayer.remove_orphan_tx(&tx_hash);
                        }
                        if is_malformed(&err) {
                            ban_malformed(nc, &err, peer);
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

#[allow(unused_variables)]
fn ban_malformed(nc: &(dyn CKBProtocolContext + Sync), error: &Error, peer: PeerIndex) {
    #[cfg(feature = "with_sentry")]
    with_scope(
        |scope| scope.set_fingerprint(Some(&["ckb-sync", "relay-invalid-tx"])),
        || {
            capture_message(
                &format!(
                    "Ban peer {} for {} seconds, reason: \
                     relay invalid tx, error: {:?}",
                    peer,
                    DEFAULT_BAN_TIME.as_secs(),
                    error
                ),
                Level::Info,
            )
        },
    );
    nc.ban_peer(
        peer,
        DEFAULT_BAN_TIME,
        String::from("send us an invalid transaction"),
    );
}

fn broadcast_tx(relayer: &Relayer, tx_hash: packed::Byte32, peer: PeerIndex) {
    let mut map = relayer.shared().state().tx_hashes();
    let set = map.entry(peer).or_insert_with(LinkedHashSet::default);
    set.insert(tx_hash);
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
