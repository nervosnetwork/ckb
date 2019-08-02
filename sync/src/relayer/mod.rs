mod block_proposal_process;
mod block_transactions_process;
mod block_transactions_verifier;
pub mod compact_block;
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
use self::compact_block::CompactBlock;
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
use ckb_core::block::{Block, BlockBuilder};
use ckb_core::transaction::{ProposalShortId, Transaction};
use ckb_core::uncle::UncleBlock;
use ckb_logger::{debug_target, info_target, trace_target};
use ckb_network::{CKBProtocolContext, CKBProtocolHandler, PeerIndex, TargetSession};
use ckb_protocol::error::Error as ProtocalError;
use ckb_protocol::{cast, get_root, RelayMessage, RelayPayload};
use ckb_store::ChainStore;
use ckb_tx_pool_executor::TxPoolExecutor;
use failure::Error as FailureError;
use faketime::unix_time_as_millis;
use flatbuffers::FlatBufferBuilder;
use fnv::{FnvHashMap, FnvHashSet};
use numext_fixed_hash::H256;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const TX_PROPOSAL_TOKEN: u64 = 0;
pub const ASK_FOR_TXS_TOKEN: u64 = 1;
pub const TX_HASHES_TOKEN: u64 = 2;

pub const MAX_RELAY_PEERS: usize = 128;

pub struct Relayer<CS> {
    chain: ChainController,
    pub(crate) shared: Arc<SyncSharedState<CS>>,
    pub(crate) tx_pool_executor: Arc<TxPoolExecutor<CS>>,
}

impl<CS: ChainStore> Clone for Relayer<CS> {
    fn clone(&self) -> Self {
        Relayer {
            chain: self.chain.clone(),
            shared: Arc::clone(&self.shared),
            tx_pool_executor: Arc::clone(&self.tx_pool_executor),
        }
    }
}

impl<CS: ChainStore + 'static> Relayer<CS> {
    pub fn new(chain: ChainController, shared: Arc<SyncSharedState<CS>>) -> Self {
        let tx_pool_executor = Arc::new(TxPoolExecutor::new(shared.shared().clone()));
        Relayer {
            chain,
            shared,
            tx_pool_executor,
        }
    }

    pub fn shared(&self) -> &Arc<SyncSharedState<CS>> {
        &self.shared
    }

    fn try_process(
        &self,
        nc: Arc<dyn CKBProtocolContext + Sync>,
        peer: PeerIndex,
        message: RelayMessage,
    ) -> Result<(), FailureError> {
        match message.payload_type() {
            RelayPayload::CompactBlock => {
                CompactBlockProcess::new(
                    &cast!(message.payload_as_compact_block())?,
                    self,
                    nc,
                    peer,
                )
                .execute()?;
            }
            RelayPayload::RelayTransactions => {
                TransactionsProcess::new(
                    &cast!(message.payload_as_relay_transactions())?,
                    self,
                    nc,
                    peer,
                )
                .execute()?;
            }
            RelayPayload::RelayTransactionHashes => {
                TransactionHashesProcess::new(
                    &cast!(message.payload_as_relay_transaction_hashes())?,
                    self,
                    peer,
                )
                .execute()?;
            }
            RelayPayload::GetRelayTransactions => {
                GetTransactionsProcess::new(
                    &cast!(message.payload_as_get_relay_transactions())?,
                    self,
                    nc,
                    peer,
                )
                .execute()?;
            }
            RelayPayload::GetBlockTransactions => {
                GetBlockTransactionsProcess::new(
                    &cast!(message.payload_as_get_block_transactions())?,
                    self,
                    nc,
                    peer,
                )
                .execute()?;
            }
            RelayPayload::BlockTransactions => {
                BlockTransactionsProcess::new(
                    &cast!(message.payload_as_block_transactions())?,
                    self,
                    nc,
                    peer,
                )
                .execute()?;
            }
            RelayPayload::GetBlockProposal => {
                GetBlockProposalProcess::new(
                    &cast!(message.payload_as_get_block_proposal())?,
                    self,
                    nc,
                    peer,
                )
                .execute()?;
            }
            RelayPayload::BlockProposal => {
                BlockProposalProcess::new(&cast!(message.payload_as_block_proposal())?, self, nc)
                    .execute()?;
            }
            RelayPayload::NONE => {
                cast!(None)?;
            }
        }
        Ok(())
    }

    fn process(
        &self,
        nc: Arc<dyn CKBProtocolContext + Sync>,
        peer: PeerIndex,
        message: RelayMessage,
    ) {
        if let Err(err) = self.try_process(Arc::clone(&nc), peer, message) {
            if let Some(&Error::Misbehavior(ref e)) = err.downcast_ref() {
                debug_target!(crate::LOG_TARGET_RELAY, "try_process error {}", e);
                nc.ban_peer(peer, BAD_MESSAGE_BAN_TIME);
                return;
            }
            if let Some(&ProtocalError::Malformed) = err.downcast_ref() {
                debug_target!(
                    crate::LOG_TARGET_RELAY,
                    "try_process error {}",
                    ProtocalError::Malformed
                );
                nc.ban_peer(peer, BAD_MESSAGE_BAN_TIME);
            }
        }
    }

    pub fn request_proposal_txs(
        &self,
        nc: &CKBProtocolContext,
        peer: PeerIndex,
        block: &CompactBlock,
    ) {
        let proposals = block
            .proposals
            .iter()
            .chain(block.uncles.iter().flat_map(UncleBlock::proposals));
        let fresh_proposals: Vec<ProposalShortId> = {
            let chain_state = self.shared.lock_chain_state();
            let tx_pool = chain_state.tx_pool();
            proposals
                .filter(|id| !tx_pool.contains_proposal_id(id))
                .cloned()
                .collect()
        };
        let to_ask_proposals: Vec<ProposalShortId> = self
            .shared()
            .insert_inflight_proposals(fresh_proposals.clone())
            .into_iter()
            .zip(fresh_proposals)
            .filter_map(|(firstly_in, id)| if firstly_in { Some(id) } else { None })
            .collect();
        if !to_ask_proposals.is_empty() {
            let fbb = &mut FlatBufferBuilder::new();
            let message =
                RelayMessage::build_get_block_proposal(fbb, block.header.hash(), &to_ask_proposals);
            fbb.finish(message, None);

            if let Err(err) = nc.send_message_to(peer, fbb.finished_data().into()) {
                debug_target!(
                    crate::LOG_TARGET_RELAY,
                    "relayer send GetBlockProposal error {:?}",
                    err,
                );
                self.shared().remove_inflight_proposals(&to_ask_proposals);
            }
        }
    }

    pub fn accept_block(&self, nc: &CKBProtocolContext, peer: PeerIndex, block: Block) {
        if self
            .shared()
            .contains_block_status(block.header().hash(), BlockStatus::BLOCK_STORED)
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
                "[block_relay] relayer accept_block {:x} {}",
                boxed.header().hash(),
                unix_time_as_millis()
            );
            let block_hash = boxed.header().hash();
            self.shared.remove_header_view(&block_hash);
            let fbb = &mut FlatBufferBuilder::new();
            let message = RelayMessage::build_compact_block(fbb, &boxed, &HashSet::new());
            fbb.finish(message, None);
            let data = fbb.finished_data().into();

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

    pub fn reconstruct_block(
        &self,
        compact_block: &CompactBlock,
        transactions: Vec<Transaction>,
    ) -> Result<Block, Vec<usize>> {
        debug_target!(
            crate::LOG_TARGET_RELAY,
            "start block reconstruction, block hash: {:#x}, transactions len: {}",
            compact_block.header.hash(),
            transactions.len(),
        );

        let mut short_ids_set: HashSet<&ProposalShortId> = compact_block.short_ids.iter().collect();

        let mut txs_map: FnvHashMap<ProposalShortId, Transaction> = transactions
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
            let chain_state = self.shared().lock_chain_state();
            short_ids_set.into_iter().for_each(|short_id| {
                if let Some(tx) = chain_state.get_tx_from_pool_or_store(&short_id) {
                    txs_map.insert(*short_id, tx);
                }
            })
        }

        let txs_len = compact_block.prefilled_transactions.len() + compact_block.short_ids.len();
        let mut block_transactions: Vec<Option<Transaction>> = Vec::with_capacity(txs_len);

        let short_ids_iter = &mut compact_block.short_ids.iter();
        // fill transactions gap
        compact_block.prefilled_transactions.iter().for_each(|pt| {
            let gap = pt.index - block_transactions.len();
            if gap > 0 {
                short_ids_iter
                    .take(gap)
                    .for_each(|short_id| block_transactions.push(txs_map.remove(short_id)));
            }
            block_transactions.push(Some(pt.transaction.clone()));
        });

        // append remain transactions
        short_ids_iter.for_each(|short_id| block_transactions.push(txs_map.remove(short_id)));

        let missing = block_transactions.iter().any(Option::is_none);

        if !missing {
            let txs = block_transactions
                .into_iter()
                .collect::<Option<Vec<_>>>()
                .expect("missing checked, should not fail");
            let block = BlockBuilder::default()
                .header(compact_block.header.clone())
                .uncles(compact_block.uncles.clone())
                .transactions(txs)
                .proposals(compact_block.proposals.clone())
                .build();

            debug_target!(
                crate::LOG_TARGET_RELAY,
                "finish block reconstruction, block hash: {:#x}",
                compact_block.header.hash(),
            );

            Ok(block)
        } else {
            let missing_indexes: Vec<usize> = block_transactions
                .iter()
                .enumerate()
                .filter_map(|(i, t)| if t.is_none() { Some(i) } else { None })
                .collect();

            debug_target!(
                crate::LOG_TARGET_RELAY,
                "block reconstruction failed, block hash: {:#x}, missing: {}, total: {}",
                compact_block.header.hash(),
                missing_indexes.len(),
                compact_block.short_ids.len(),
            );

            Err(missing_indexes)
        }
    }

    fn prune_tx_proposal_request(&self, nc: &CKBProtocolContext) {
        let get_block_proposals = self.shared().clear_get_block_proposals();
        let mut peer_txs = FnvHashMap::default();
        {
            let chain_state = self.shared.lock_chain_state();
            let tx_pool = chain_state.tx_pool();
            for (id, peer_indices) in get_block_proposals.into_iter() {
                if let Some(tx) = tx_pool.get_tx(&id) {
                    for peer_index in peer_indices {
                        let tx_set = peer_txs.entry(peer_index).or_insert_with(Vec::new);
                        tx_set.push(tx.clone());
                    }
                }
            }
        }

        for (peer_index, txs) in peer_txs {
            let fbb = &mut FlatBufferBuilder::new();
            let message =
                RelayMessage::build_block_proposal(fbb, &txs.into_iter().collect::<Vec<_>>());
            fbb.finish(message, None);
            if let Err(err) = nc.send_message_to(peer_index, fbb.finished_data().into()) {
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
            }

            let fbb = &mut FlatBufferBuilder::new();
            let message = RelayMessage::build_get_transactions(fbb, &tx_hashes);
            fbb.finish(message, None);
            let data = fbb.finished_data().into();
            if let Err(err) = nc.send_message_to(*peer, data) {
                debug_target!(
                    crate::LOG_TARGET_RELAY,
                    "relayer send Transaction error: {:?}",
                    err,
                );
                break;
            }
        }
    }

    // Send bulk of tx hashes to selected peers
    pub fn send_bulk_of_tx_hashes(&self, nc: &CKBProtocolContext) {
        let mut selected: FnvHashMap<PeerIndex, FnvHashSet<H256>> = FnvHashMap::default();
        {
            let peer_tx_hashes = self.shared.take_tx_hashes();
            let mut known_txs = self.shared.known_txs();

            for (peer_index, tx_hashes) in peer_tx_hashes.into_iter() {
                for tx_hash in tx_hashes {
                    let peers: Vec<PeerIndex> = nc
                        .connected_peers()
                        .into_iter()
                        .filter(|target_peer| {
                            known_txs.insert(*target_peer, tx_hash.clone())
                                && (peer_index != *target_peer)
                        })
                        .take(MAX_RELAY_PEERS)
                        .collect();
                    for peer in peers {
                        let hashes = selected.entry(peer).or_insert_with(FnvHashSet::default);
                        hashes.insert(tx_hash.clone());
                    }
                }
            }
        };

        for (peer, hashes) in selected {
            let fbb = &mut FlatBufferBuilder::new();
            let vec: Vec<H256> = hashes.into_iter().collect();
            let message = RelayMessage::build_transaction_hashes(fbb, &vec);
            fbb.finish(message, None);
            let data = fbb.finished_data().into();
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

impl<CS: ChainStore + 'static> CKBProtocolHandler for Relayer<CS> {
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

        let msg = match get_root::<RelayMessage>(&data) {
            Ok(msg) => msg,
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
            "received msg {:?} from {}",
            msg.payload_type(),
            peer_index
        );
        let start_time = Instant::now();
        self.process(nc, peer_index, msg);
        debug_target!(
            crate::LOG_TARGET_RELAY,
            "process message={:?}, peer={}, cost={:?}",
            msg.payload_type(),
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
