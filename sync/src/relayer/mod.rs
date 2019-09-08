mod block_proposal_process;
mod block_transactions_process;
mod block_transactions_verifier;
mod compact_block_process;
mod compact_block_verifier;
mod error;
mod get_block_proposal_process;
mod get_block_transactions_process;
mod get_transactions_process;
#[cfg(test)]
mod tests;
mod transaction_hashes_process;
mod transactions_process;

use self::block_proposal_process::BlockProposalProcess;
use self::block_transactions_process::BlockTransactionsProcess;
use self::compact_block_process::CompactBlockProcess;
pub use self::error::{Error, Misbehavior};
use self::get_block_proposal_process::GetBlockProposalProcess;
use self::get_block_transactions_process::GetBlockTransactionsProcess;
use self::get_transactions_process::GetTransactionsProcess;
use self::transaction_hashes_process::TransactionHashesProcess;
use self::transactions_process::TransactionsProcess;
use crate::block_status::BlockStatus;
use crate::types::SyncSharedState;
use crate::BAD_MESSAGE_BAN_TIME;
use ckb_chain::chain::ChainController;
use ckb_logger::{debug_target, info_target, trace_target};
use ckb_network::{CKBProtocolContext, CKBProtocolHandler, PeerIndex, TargetSession};
use ckb_types::{
    core,
    packed::{self, Byte32, ProposalShortId},
    prelude::*,
};
use failure::err_msg;
use failure::Error as FailureError;
use faketime::unix_time_as_millis;
use futures::future::Future;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const TX_PROPOSAL_TOKEN: u64 = 0;
pub const ASK_FOR_TXS_TOKEN: u64 = 1;
pub const TX_HASHES_TOKEN: u64 = 2;

pub const MAX_RELAY_PEERS: usize = 128;

#[derive(Debug, Eq, PartialEq)]
pub enum ReconstructionError {
    MissingIndexes(Vec<usize>),
    InvalidTransactionRoot,
    Collision,
}

#[derive(Clone)]
pub struct Relayer {
    chain: ChainController,
    pub(crate) shared: Arc<SyncSharedState>,
}

impl Relayer {
    pub fn new(chain: ChainController, shared: Arc<SyncSharedState>) -> Self {
        Relayer { chain, shared }
    }

    pub fn shared(&self) -> &Arc<SyncSharedState> {
        &self.shared
    }

    fn try_process<'r>(
        &self,
        nc: Arc<dyn CKBProtocolContext + Sync>,
        peer: PeerIndex,
        message: packed::RelayMessageUnionReader<'r>,
    ) -> Result<(), FailureError> {
        match message {
            packed::RelayMessageUnionReader::CompactBlock(reader) => {
                CompactBlockProcess::new(reader, self, nc, peer).execute()?;
            }
            packed::RelayMessageUnionReader::RelayTransactions(reader) => {
                if reader.check_data() {
                    TransactionsProcess::new(reader, self, nc, peer).execute()?;
                } else {
                    Err(err_msg("RelayTransactions: invalid data"))?;
                }
            }
            packed::RelayMessageUnionReader::RelayTransactionHashes(reader) => {
                TransactionHashesProcess::new(reader, self, peer).execute()?;
            }
            packed::RelayMessageUnionReader::GetRelayTransactions(reader) => {
                GetTransactionsProcess::new(reader, self, nc, peer).execute()?;
            }
            packed::RelayMessageUnionReader::GetBlockTransactions(reader) => {
                GetBlockTransactionsProcess::new(reader, self, nc, peer).execute()?;
            }
            packed::RelayMessageUnionReader::BlockTransactions(reader) => {
                if reader.check_data() {
                    BlockTransactionsProcess::new(reader, self, nc, peer).execute()?;
                } else {
                    Err(err_msg("BlockTransactions: invalid data"))?;
                }
            }
            packed::RelayMessageUnionReader::GetBlockProposal(reader) => {
                GetBlockProposalProcess::new(reader, self, nc, peer).execute()?;
            }
            packed::RelayMessageUnionReader::BlockProposal(reader) => {
                BlockProposalProcess::new(reader, self).execute()?;
            }
            packed::RelayMessageUnionReader::NotSet => Err(err_msg("invalid data"))?,
        }
        Ok(())
    }

    fn process<'r>(
        &self,
        nc: Arc<dyn CKBProtocolContext + Sync>,
        peer: PeerIndex,
        message: packed::RelayMessageUnionReader<'r>,
    ) {
        if let Err(err) = self.try_process(Arc::clone(&nc), peer, message) {
            if let Some(&Error::Misbehavior(ref e)) = err.downcast_ref() {
                debug_target!(crate::LOG_TARGET_RELAY, "try_process error {}", e);
                nc.ban_peer(peer, BAD_MESSAGE_BAN_TIME);
                return;
            }
        }
    }

    pub fn request_proposal_txs(
        &self,
        nc: &CKBProtocolContext,
        peer: PeerIndex,
        block: &packed::CompactBlock,
    ) {
        let proposals = block.proposals().into_iter().chain(
            block
                .uncles()
                .into_iter()
                .flat_map(|u| u.proposals().into_iter()),
        );
        let fresh_proposals = {
            let tx_pool = self.shared.shared().tx_pool_controller();
            // TODO: error handle
            tx_pool
                .fresh_proposals_filter(proposals.collect())
                .unwrap()
                .wait()
                .unwrap()
        };

        let to_ask_proposals: Vec<ProposalShortId> = self
            .shared()
            .insert_inflight_proposals(fresh_proposals.clone())
            .into_iter()
            .zip(fresh_proposals)
            .filter_map(|(firstly_in, id)| if firstly_in { Some(id) } else { None })
            .collect();
        if !to_ask_proposals.is_empty() {
            let content = packed::GetBlockProposal::new_builder()
                .block_hash(block.calc_header_hash())
                .proposals(to_ask_proposals.clone().pack())
                .build();
            let message = packed::RelayMessage::new_builder().set(content).build();
            let data = message.as_slice().into();

            if let Err(err) = nc.send_message_to(peer, data) {
                debug_target!(
                    crate::LOG_TARGET_RELAY,
                    "relayer send GetBlockProposal error {:?}",
                    err,
                );
                self.shared().remove_inflight_proposals(&to_ask_proposals);
            }
        }
    }

    pub fn accept_block(&self, nc: &CKBProtocolContext, peer: PeerIndex, block: core::BlockView) {
        if self
            .shared()
            .contains_block_status(&block.hash(), BlockStatus::BLOCK_STORED)
        {
            return;
        }

        let boxed = Arc::new(block);
        if self
            .shared()
            .insert_new_block(&self.chain, peer, Arc::clone(&boxed))
            .unwrap_or(false)
        {
            debug_target!(
                crate::LOG_TARGET_RELAY,
                "[block_relay] relayer accept_block {} {}",
                boxed.header().hash(),
                unix_time_as_millis()
            );
            let block_hash = boxed.hash();
            self.shared.remove_header_view(&block_hash);
            let cb = packed::CompactBlock::build_from_block(&boxed, &HashSet::new());
            let message = packed::RelayMessage::new_builder().set(cb).build();
            let data = message.as_slice().into();

            let selected_peers: Vec<PeerIndex> = nc
                .connected_peers()
                .into_iter()
                .filter(|target_peer| peer != *target_peer)
                .take(MAX_RELAY_PEERS)
                .collect();
            if let Err(err) = nc.quick_filter_broadcast(TargetSession::Multi(selected_peers), data)
            {
                debug_target!(
                    crate::LOG_TARGET_RELAY,
                    "relayer send block when accept block error: {:?}",
                    err,
                );
            }
        }
    }

    // nodes should attempt to reconstruct the full block by taking the prefilledtxn transactions
    // from the original CompactBlock message and placing them in the marked positions,
    // then for each short transaction ID from the original compact_block message, in order,
    // find the corresponding transaction either from the BlockTransactions message or
    // from other sources and place it in the first available position in the block
    // then once the block has been reconstructed, it shall be processed as normal,
    // keeping in mind that short_ids are expected to occasionally collide,
    // and that nodes must not be penalized for such collisions, wherever they appear.
    pub fn reconstruct_block(
        &self,
        compact_block: &packed::CompactBlock,
        transactions: Vec<core::TransactionView>,
    ) -> Result<core::BlockView, ReconstructionError> {
        let block_txs_len = transactions.len();
        debug_target!(
            crate::LOG_TARGET_RELAY,
            "start block reconstruction, block hash: {}, transactions len: {}",
            compact_block.calc_header_hash(),
            block_txs_len,
        );

        let mut short_ids_set: HashSet<ProposalShortId> =
            compact_block.short_ids().into_iter().collect();

        let mut txs_map: HashMap<ProposalShortId, core::TransactionView> = transactions
            .into_iter()
            .filter_map(|tx| {
                let short_id = tx.proposal_short_id();
                if short_ids_set.remove(&short_id) {
                    Some((short_id, tx))
                } else {
                    None
                }
            })
            .collect();

        if !short_ids_set.is_empty() {
            let tx_pool = self.shared.shared().tx_pool_controller();
            // TODO: error handle
            let txs = tx_pool.fetch_txs(short_ids_set).unwrap().wait().unwrap();
            txs_map.extend(txs.into_iter());
        }

        let txs_len = compact_block.txs_len();
        let mut block_transactions: Vec<Option<core::TransactionView>> =
            Vec::with_capacity(txs_len);

        let short_ids_iter = &mut compact_block.short_ids().into_iter();
        // fill transactions gap
        compact_block
            .prefilled_transactions()
            .into_iter()
            .for_each(|pt| {
                let index: usize = pt.index().unpack();
                let gap = index - block_transactions.len();
                if gap > 0 {
                    short_ids_iter
                        .take(gap)
                        .for_each(|short_id| block_transactions.push(txs_map.remove(&short_id)));
                }
                block_transactions.push(Some(pt.transaction().into_view()));
            });

        // append remain transactions
        short_ids_iter.for_each(|short_id| block_transactions.push(txs_map.remove(&short_id)));

        let missing = block_transactions.iter().any(Option::is_none);

        if !missing {
            let txs = block_transactions
                .into_iter()
                .collect::<Option<Vec<_>>>()
                .expect("missing checked, should not fail");
            let block = packed::Block::new_builder()
                .header(compact_block.header())
                .uncles(compact_block.uncles())
                .transactions(txs.into_iter().map(|tx| tx.data()).pack())
                .proposals(compact_block.proposals())
                .build()
                .into_view();

            debug_target!(
                crate::LOG_TARGET_RELAY,
                "finish block reconstruction, block hash: {}",
                compact_block.calc_header_hash(),
            );

            if compact_block.header().raw().transactions_root() != block.transactions_root() {
                if compact_block.short_ids().is_empty()
                    || compact_block.short_ids().len() == block_txs_len
                {
                    return Err(ReconstructionError::InvalidTransactionRoot);
                } else {
                    return Err(ReconstructionError::Collision);
                }
            }

            Ok(block)
        } else {
            let missing_indexes: Vec<usize> = block_transactions
                .iter()
                .enumerate()
                .filter_map(|(i, t)| if t.is_none() { Some(i) } else { None })
                .collect();

            debug_target!(
                crate::LOG_TARGET_RELAY,
                "block reconstruction failed, block hash: {}, missing: {}, total: {}",
                compact_block.calc_header_hash(),
                missing_indexes.len(),
                compact_block.short_ids().len(),
            );

            Err(ReconstructionError::MissingIndexes(missing_indexes))
        }
    }

    fn prune_tx_proposal_request(&self, nc: &CKBProtocolContext) {
        let get_block_proposals = self.shared().clear_get_block_proposals();
        let tx_pool = self.shared.shared().tx_pool_controller();
        // TODO: error handle
        let txs = tx_pool
            .fetch_txs(get_block_proposals.keys().cloned().collect())
            .unwrap()
            .wait()
            .unwrap();
        let mut peer_txs = HashMap::new();
        for (id, peer_indices) in get_block_proposals.into_iter() {
            if let Some(tx) = txs.get(&id) {
                for peer_index in peer_indices {
                    let tx_set = peer_txs.entry(peer_index).or_insert_with(Vec::new);
                    tx_set.push(tx.clone());
                }
            }
        }

        for (peer_index, txs) in peer_txs {
            let content = packed::BlockProposal::new_builder()
                .transactions(txs.into_iter().map(|tx| tx.data()).pack())
                .build();
            let message = packed::RelayMessage::new_builder().set(content).build();
            let data = message.as_slice().into();
            if let Err(err) = nc.send_message_to(peer_index, data) {
                debug_target!(
                    crate::LOG_TARGET_RELAY,
                    "relayer send BlockProposal error: {:?}",
                    err,
                );
            }
        }
    }

    // Ask for relay transaction by hash from all peers
    pub fn ask_for_txs(&self, nc: &CKBProtocolContext) {
        for (peer, peer_state) in self.shared().peers().state.write().iter_mut() {
            let tx_hashes = peer_state
                .pop_ask_for_txs()
                .into_iter()
                .filter(|tx_hash| {
                    let already_known = self.shared().already_known_tx(&tx_hash);
                    if already_known {
                        // Remove tx_hash from `tx_ask_for_set`
                        peer_state.remove_ask_for_tx(&tx_hash);
                    }
                    !already_known
                })
                .collect::<Vec<_>>();
            if !tx_hashes.is_empty() {
                debug_target!(
                    crate::LOG_TARGET_RELAY,
                    "Send get transaction ({} hashes) to {}",
                    tx_hashes.len(),
                    peer,
                );
                let content = packed::GetRelayTransactions::new_builder()
                    .tx_hashes(tx_hashes.pack())
                    .build();
                let message = packed::RelayMessage::new_builder().set(content).build();
                let data = message.as_slice().into();
                if let Err(err) = nc.send_message_to(*peer, data) {
                    debug_target!(
                        crate::LOG_TARGET_RELAY,
                        "relayer send Transaction error: {:?}",
                        err,
                    );
                }
            }
        }
    }

    // Send bulk of tx hashes to selected peers
    pub fn send_bulk_of_tx_hashes(&self, nc: &CKBProtocolContext) {
        let mut selected: HashMap<PeerIndex, HashSet<Byte32>> = HashMap::default();
        {
            let peer_tx_hashes = self.shared.take_tx_hashes();
            let mut known_txs = self.shared.known_txs();

            for (peer_index, tx_hashes) in peer_tx_hashes.into_iter() {
                for tx_hash in tx_hashes {
                    for peer in nc
                        .connected_peers()
                        .into_iter()
                        .filter(|target_peer| {
                            known_txs.insert(*target_peer, tx_hash.clone())
                                && (peer_index != *target_peer)
                        })
                        .take(MAX_RELAY_PEERS)
                    {
                        let hashes = selected.entry(peer).or_insert_with(HashSet::default);
                        hashes.insert(tx_hash.clone());
                    }
                }
            }
        };

        for (peer, hashes) in selected {
            let content = packed::RelayTransactionHashes::new_builder()
                .tx_hashes(hashes.pack())
                .build();
            let message = packed::RelayMessage::new_builder().set(content).build();
            let data = message.as_slice().into();
            if let Err(err) = nc.filter_broadcast(TargetSession::Single(peer), data) {
                debug_target!(
                    crate::LOG_TARGET_RELAY,
                    "relayer send TransactionHashes error: {:?}",
                    err,
                );
            }
        }
    }
}

impl CKBProtocolHandler for Relayer {
    fn init(&mut self, nc: Arc<dyn CKBProtocolContext + Sync>) {
        nc.set_notify(Duration::from_millis(100), TX_PROPOSAL_TOKEN)
            .expect("set_notify at init is ok");
        nc.set_notify(Duration::from_millis(100), ASK_FOR_TXS_TOKEN)
            .expect("set_notify at init is ok");
        nc.set_notify(Duration::from_millis(300), TX_HASHES_TOKEN)
            .expect("set_notify at init is ok");
    }

    fn received(
        &mut self,
        nc: Arc<dyn CKBProtocolContext + Sync>,
        peer_index: PeerIndex,
        data: bytes::Bytes,
    ) {
        // If self is in the IBD state, don't process any relayer message.
        if self.shared.is_initial_block_download() {
            return;
        }

        let msg = match packed::RelayMessage::from_slice(&data) {
            Ok(msg) => msg.to_enum(),
            _ => {
                info_target!(
                    crate::LOG_TARGET_RELAY,
                    "Peer {} sends us a malformed message",
                    peer_index
                );
                nc.ban_peer(peer_index, BAD_MESSAGE_BAN_TIME);
                return;
            }
        };

        debug_target!(
            crate::LOG_TARGET_RELAY,
            "received msg {} from {}",
            msg.item_name(),
            peer_index
        );
        let sentry_hub = sentry::Hub::current();
        let _scope_guard = sentry_hub.push_scope();
        sentry_hub.configure_scope(|scope| {
            scope.set_tag("p2p.protocol", "relayer");
            scope.set_tag("p2p.message", msg.item_name());
        });

        let start_time = Instant::now();
        self.process(nc, peer_index, msg.as_reader());
        debug_target!(
            crate::LOG_TARGET_RELAY,
            "process message={}, peer={}, cost={:?}",
            msg.item_name(),
            peer_index,
            start_time.elapsed(),
        );
    }

    fn connected(
        &mut self,
        _nc: Arc<dyn CKBProtocolContext + Sync>,
        peer_index: PeerIndex,
        version: &str,
    ) {
        info_target!(
            crate::LOG_TARGET_RELAY,
            "RelayProtocol({}).connected peer={}",
            version,
            peer_index
        );
        // do nothing
    }

    fn disconnected(&mut self, _nc: Arc<dyn CKBProtocolContext + Sync>, peer_index: PeerIndex) {
        info_target!(
            crate::LOG_TARGET_RELAY,
            "RelayProtocol.disconnected peer={}",
            peer_index
        );
        // TODO
    }

    fn notify(&mut self, nc: Arc<dyn CKBProtocolContext + Sync>, token: u64) {
        // If self is in the IBD state, don't trigger any relayer notify.
        if self.shared.is_initial_block_download() {
            return;
        }

        let start_time = Instant::now();
        trace_target!(crate::LOG_TARGET_RELAY, "start notify token={}", token);
        match token {
            TX_PROPOSAL_TOKEN => self.prune_tx_proposal_request(nc.as_ref()),
            ASK_FOR_TXS_TOKEN => self.ask_for_txs(nc.as_ref()),
            TX_HASHES_TOKEN => self.send_bulk_of_tx_hashes(nc.as_ref()),
            _ => unreachable!(),
        }
        trace_target!(
            crate::LOG_TARGET_RELAY,
            "finished notify token={} cost={:?}",
            token,
            start_time.elapsed()
        );
    }
}
