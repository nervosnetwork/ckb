mod block_proposal_process;
mod block_transactions_process;
pub mod compact_block;
mod compact_block_process;
mod compact_block_verifier;
mod error;
mod get_block_proposal_process;
mod get_block_transactions_process;
mod get_transaction_process;
#[cfg(test)]
mod tests;
mod transaction_hash_process;
mod transaction_process;

use self::block_proposal_process::BlockProposalProcess;
use self::block_transactions_process::BlockTransactionsProcess;
use self::compact_block::CompactBlock;
use self::compact_block_process::CompactBlockProcess;
pub use self::error::Error;
use self::get_block_proposal_process::GetBlockProposalProcess;
use self::get_block_transactions_process::GetBlockTransactionsProcess;
use self::get_transaction_process::GetTransactionProcess;
use self::transaction_hash_process::TransactionHashProcess;
use self::transaction_process::TransactionProcess;
use crate::relayer::compact_block::ShortTransactionID;
use crate::types::{Peers, SyncSharedState};
use crate::BAD_MESSAGE_BAN_TIME;
use ckb_chain::chain::ChainController;
use ckb_core::block::{Block, BlockBuilder};
use ckb_core::transaction::{ProposalShortId, Transaction};
use ckb_core::uncle::UncleBlock;
use ckb_network::{CKBProtocolContext, CKBProtocolHandler, PeerIndex, TargetSession};
use ckb_protocol::{
    cast, get_root, short_transaction_id, short_transaction_id_keys, RelayMessage, RelayPayload,
};
use ckb_shared::chain_state::ChainState;
use ckb_store::ChainStore;
use ckb_tx_pool_executor::TxPoolExecutor;
use ckb_util::Mutex;
use failure::Error as FailureError;
use faketime::unix_time_as_millis;
use flatbuffers::FlatBufferBuilder;
use fnv::{FnvHashMap, FnvHashSet};
use log::{debug, info, trace};
use lru_cache::LruCache;
use numext_fixed_hash::H256;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const TX_PROPOSAL_TOKEN: u64 = 0;
pub const ASK_FOR_TXS_TOKEN: u64 = 1;

pub const MAX_RELAY_PEERS: usize = 128;
pub const TX_FILTER_SIZE: usize = 50000;
pub const TX_ASKED_SIZE: usize = TX_FILTER_SIZE;

pub struct Relayer<CS> {
    chain: ChainController,
    pub(crate) shared: Arc<SyncSharedState<CS>>,
    pub(crate) state: Arc<RelayState>,
    // TODO refactor shared Peers struct with Synchronizer
    peers: Arc<Peers>,
    pub(crate) tx_pool_executor: Arc<TxPoolExecutor<CS>>,
}

impl<CS: ChainStore> Clone for Relayer<CS> {
    fn clone(&self) -> Self {
        Relayer {
            chain: self.chain.clone(),
            shared: Arc::clone(&self.shared),
            state: Arc::clone(&self.state),
            peers: Arc::clone(&self.peers),
            tx_pool_executor: Arc::clone(&self.tx_pool_executor),
        }
    }
}

impl<CS: ChainStore + 'static> Relayer<CS> {
    pub fn new(
        chain: ChainController,
        shared: Arc<SyncSharedState<CS>>,
        peers: Arc<Peers>,
    ) -> Self {
        let tx_pool_executor = Arc::new(TxPoolExecutor::new(shared.shared().clone()));
        Relayer {
            chain,
            shared,
            state: Arc::new(RelayState::default()),
            peers,
            tx_pool_executor,
        }
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
            RelayPayload::RelayTransaction => {
                TransactionProcess::new(
                    &cast!(message.payload_as_relay_transaction())?,
                    self,
                    nc,
                    peer,
                )
                .execute()?;
            }
            RelayPayload::RelayTransactionHash => {
                TransactionHashProcess::new(
                    &cast!(message.payload_as_relay_transaction_hash())?,
                    self,
                    nc,
                    peer,
                )
                .execute()?;
            }
            RelayPayload::GetRelayTransaction => {
                GetTransactionProcess::new(
                    &cast!(message.payload_as_get_relay_transaction())?,
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
            debug!(target: "relay", "try_process error {}", err);
            nc.ban_peer(peer, BAD_MESSAGE_BAN_TIME);
        }
    }

    pub fn request_proposal_txs(
        &self,
        chain_state: &ChainState<CS>,
        nc: &CKBProtocolContext,
        peer: PeerIndex,
        block: &CompactBlock,
    ) {
        let mut inflight = self.state.inflight_proposals.lock();
        let unknown_ids = block
            .proposals
            .iter()
            .chain(block.uncles.iter().flat_map(UncleBlock::proposals))
            .filter(|x| !chain_state.contains_proposal_id(x) && inflight.insert(**x))
            .cloned()
            .collect::<Vec<_>>();

        if !unknown_ids.is_empty() {
            let fbb = &mut FlatBufferBuilder::new();
            let message =
                RelayMessage::build_get_block_proposal(fbb, block.header.number(), &unknown_ids);
            fbb.finish(message, None);

            nc.send_message_to(peer, fbb.finished_data().into());
        }
    }

    pub fn accept_block(&self, nc: &CKBProtocolContext, peer: PeerIndex, block: &Arc<Block>) {
        let ret = self.chain.process_block(Arc::clone(&block));

        if ret.is_ok() {
            debug!(target: "relay", "[block_relay] relayer accept_block {:x} {}", block.header().hash(), unix_time_as_millis());
            let block_hash = block.header().hash();
            self.shared.remove_header_view(&block_hash);
            let fbb = &mut FlatBufferBuilder::new();
            let message = RelayMessage::build_compact_block(fbb, block, &HashSet::new());
            fbb.finish(message, None);
            let data = fbb.finished_data().into();

            let mut known_blocks = self.peers.known_blocks.lock();
            let selected_peers: Vec<PeerIndex> = nc
                .connected_peers()
                .into_iter()
                .filter(|target_peer| {
                    known_blocks.insert(*target_peer, block_hash.clone()) && (peer != *target_peer)
                })
                .take(MAX_RELAY_PEERS)
                .collect();
            nc.quick_filter_broadcast(TargetSession::Multi(selected_peers), data);
        } else {
            debug!(target: "relay", "accept_block verify error {:?}", ret);
        }
    }

    pub fn reconstruct_block(
        &self,
        chain_state: &ChainState<CS>,
        compact_block: &CompactBlock,
        transactions: Vec<Transaction>,
    ) -> Result<Block, Vec<usize>> {
        let (key0, key1) =
            short_transaction_id_keys(compact_block.header.nonce(), compact_block.nonce);
        let mut short_ids_set: HashSet<&ShortTransactionID> =
            compact_block.short_ids.iter().collect();

        let mut txs_map: FnvHashMap<ShortTransactionID, Transaction> = transactions
            .into_iter()
            .filter_map(|tx| {
                let short_id = short_transaction_id(key0, key1, &tx.witness_hash());
                if short_ids_set.remove(&short_id) {
                    Some((short_id, tx))
                } else {
                    None
                }
            })
            .collect();

        if short_ids_set.is_empty() {
            let tx_pool = chain_state.tx_pool();
            for entry in tx_pool.proposed_txs_iter() {
                let short_id = short_transaction_id(key0, key1, &entry.transaction.witness_hash());
                if short_ids_set.remove(&short_id) {
                    txs_map.insert(short_id, entry.transaction.clone());

                    // Early exit here for performance
                    if short_ids_set.is_empty() {
                        break;
                    }
                }
            }
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

            Ok(block)
        } else {
            let missing_indexes = block_transactions
                .iter()
                .enumerate()
                .filter_map(|(i, t)| if t.is_none() { Some(i) } else { None })
                .collect();
            Err(missing_indexes)
        }
    }

    fn prune_tx_proposal_request(&self, nc: &CKBProtocolContext) {
        let mut pending_proposals_request = self.state.pending_proposals_request.lock();
        let mut peer_txs = FnvHashMap::default();
        let mut remove_ids = Vec::new();
        {
            let chain_state = self.shared.chain_state().lock();
            let tx_pool = chain_state.tx_pool();
            for (id, peer_indexs) in pending_proposals_request.iter() {
                if let Some(tx) = tx_pool.get_tx(id) {
                    for peer_index in peer_indexs {
                        let tx_set = peer_txs.entry(*peer_index).or_insert_with(Vec::new);
                        tx_set.push(tx.clone());
                    }
                }
                remove_ids.push(*id);
            }
        }

        for id in remove_ids {
            pending_proposals_request.remove(&id);
        }

        for (peer_index, txs) in peer_txs {
            let fbb = &mut FlatBufferBuilder::new();
            let message =
                RelayMessage::build_block_proposal(fbb, &txs.into_iter().collect::<Vec<_>>());
            fbb.finish(message, None);
            nc.send_message_to(peer_index, fbb.finished_data().into());
        }
    }

    // Ask for relay transaction by hash from all peers
    pub fn ask_for_txs(&self, nc: &CKBProtocolContext) {
        if self.shared.is_initial_block_download() {
            trace!(target: "relay", "Do not ask for transactions when initial block download");
            return;
        }

        for (peer, peer_state) in self.peers.state.write().iter_mut() {
            let tx_hashes = peer_state
                .pop_ask_for_txs()
                .into_iter()
                .filter(|tx_hash| {
                    let already_known = self.state.already_known(&tx_hash);
                    if already_known {
                        // Remove tx_hash from `tx_ask_for_set`
                        peer_state.remove_ask_for_tx(&tx_hash);
                    }
                    !already_known
                })
                .collect::<Vec<_>>();
            if !tx_hashes.is_empty() {
                debug!(
                    target: "relay",
                    "Send get transaction ({} hashes) to {}",
                    tx_hashes.len(),
                    peer,
                );
            }
            for tx_hash in tx_hashes {
                let fbb = &mut FlatBufferBuilder::new();
                let message = RelayMessage::build_get_transaction(fbb, &tx_hash);
                fbb.finish(message, None);
                let data = fbb.finished_data().into();
                nc.send_message_to(*peer, data);
            }
        }
    }

    pub fn peers(&self) -> Arc<Peers> {
        Arc::clone(&self.peers)
    }
}

impl<CS: ChainStore + 'static> CKBProtocolHandler for Relayer<CS> {
    fn init(&mut self, nc: Arc<dyn CKBProtocolContext + Sync>) {
        nc.set_notify(Duration::from_millis(100), TX_PROPOSAL_TOKEN);
        nc.set_notify(Duration::from_millis(100), ASK_FOR_TXS_TOKEN);
    }

    fn received(
        &mut self,
        nc: Arc<dyn CKBProtocolContext + Sync>,
        peer_index: PeerIndex,
        data: bytes::Bytes,
    ) {
        let msg = match get_root::<RelayMessage>(&data) {
            Ok(msg) => msg,
            _ => {
                info!(target: "relay", "Peer {} sends us a malformed message", peer_index);
                nc.ban_peer(peer_index, BAD_MESSAGE_BAN_TIME);
                return;
            }
        };

        debug!(target: "relay", "received msg {:?} from {}", msg.payload_type(), peer_index);
        self.process(nc, peer_index, msg);
    }

    fn connected(
        &mut self,
        _nc: Arc<dyn CKBProtocolContext + Sync>,
        peer_index: PeerIndex,
        version: &str,
    ) {
        info!(target: "relay", "RelayProtocol({}).connected peer={}", version, peer_index);
        // do nothing
    }

    fn disconnected(&mut self, _nc: Arc<dyn CKBProtocolContext + Sync>, peer_index: PeerIndex) {
        info!(target: "relay", "RelayProtocol.disconnected peer={}", peer_index);
        // TODO
    }

    fn notify(&mut self, nc: Arc<dyn CKBProtocolContext + Sync>, token: u64) {
        match token {
            TX_PROPOSAL_TOKEN => self.prune_tx_proposal_request(nc.as_ref()),
            ASK_FOR_TXS_TOKEN => self.ask_for_txs(nc.as_ref()),
            _ => unreachable!(),
        }
    }
}

pub struct RelayState {
    pub pending_compact_blocks: Mutex<FnvHashMap<H256, CompactBlock>>,
    pub inflight_proposals: Mutex<FnvHashSet<ProposalShortId>>,
    pub pending_proposals_request: Mutex<FnvHashMap<ProposalShortId, FnvHashSet<PeerIndex>>>,
    pub tx_filter: Mutex<LruCache<H256, ()>>,
    pub tx_already_asked: Mutex<LruCache<H256, Instant>>,
}

impl Default for RelayState {
    fn default() -> Self {
        RelayState {
            pending_compact_blocks: Mutex::new(FnvHashMap::default()),
            inflight_proposals: Mutex::new(FnvHashSet::default()),
            pending_proposals_request: Mutex::new(FnvHashMap::default()),
            tx_filter: Mutex::new(LruCache::new(TX_FILTER_SIZE)),
            tx_already_asked: Mutex::new(LruCache::new(TX_ASKED_SIZE)),
        }
    }
}

impl RelayState {
    fn insert_tx(&self, hash: H256) {
        self.tx_already_asked.lock().remove(&hash);
        self.tx_filter.lock().insert(hash, ());
    }

    fn already_known(&self, hash: &H256) -> bool {
        self.tx_filter.lock().contains_key(hash)
    }
}
