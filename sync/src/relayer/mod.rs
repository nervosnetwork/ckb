mod block_proposal_process;
mod block_transactions_process;
pub mod compact_block;
mod compact_block_process;
mod get_block_proposal_process;
mod get_block_transactions_process;
mod transaction_process;

use self::block_proposal_process::BlockProposalProcess;
use self::block_transactions_process::BlockTransactionsProcess;
use self::compact_block::CompactBlock;
use self::compact_block_process::CompactBlockProcess;
use self::get_block_proposal_process::GetBlockProposalProcess;
use self::get_block_transactions_process::GetBlockTransactionsProcess;
use self::transaction_process::TransactionProcess;
use crate::relayer::compact_block::ShortTransactionID;
use crate::types::Peers;
use crate::BAD_MESSAGE_BAN_TIME;
use ckb_chain::chain::ChainController;
use ckb_core::block::{Block, BlockBuilder};
use ckb_core::transaction::{ProposalShortId, Transaction};
use ckb_core::uncle::UncleBlock;
use ckb_network::{CKBProtocolContext, CKBProtocolHandler, PeerIndex};
use ckb_protocol::{
    cast, get_root, short_transaction_id, short_transaction_id_keys, RelayMessage, RelayPayload,
};
use ckb_shared::chain_state::ChainState;
use ckb_shared::shared::Shared;
use ckb_shared::store::ChainStore;
use ckb_traits::ChainProvider;
use ckb_util::Mutex;
use failure::Error as FailureError;
use faketime::unix_time_as_millis;
use flatbuffers::FlatBufferBuilder;
use fnv::{FnvHashMap, FnvHashSet};
use log::{debug, info};
use lru_cache::LruCache;
use numext_fixed_hash::H256;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

pub const TX_PROPOSAL_TOKEN: u64 = 0;
pub const MAX_RELAY_PEERS: usize = 128;
pub const TX_FILTER_SIZE: usize = 1000;

pub struct Relayer<CS> {
    chain: ChainController,
    pub(crate) shared: Shared<CS>,
    state: Arc<RelayState>,
    // TODO refactor shared Peers struct with Synchronizer
    peers: Arc<Peers>,
}

impl<CS: ChainStore> Clone for Relayer<CS> {
    fn clone(&self) -> Self {
        Relayer {
            chain: self.chain.clone(),
            shared: self.shared.clone(),
            state: Arc::clone(&self.state),
            peers: Arc::clone(&self.peers),
        }
    }
}

impl<CS: ChainStore> Relayer<CS> {
    pub fn new(chain: ChainController, shared: Shared<CS>, peers: Arc<Peers>) -> Self {
        Relayer {
            chain,
            shared,
            state: Arc::new(RelayState::default()),
            peers,
        }
    }

    fn try_process(
        &self,
        nc: &CKBProtocolContext,
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
                BlockProposalProcess::new(&cast!(message.payload_as_block_proposal())?, self)
                    .execute()?;
            }
            RelayPayload::NONE => {
                cast!(None)?;
            }
        }
        Ok(())
    }

    fn process(&self, nc: &CKBProtocolContext, peer: PeerIndex, message: RelayMessage) {
        if self.try_process(nc, peer, message).is_err() {
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

            nc.send_message_to(peer, fbb.finished_data().to_vec());
        }
    }

    pub fn accept_block(&self, nc: &CKBProtocolContext, peer: PeerIndex, block: &Arc<Block>) {
        let ret = self.chain.process_block(Arc::clone(&block));

        if ret.is_ok() {
            debug!(target: "relay", "[block_relay] relayer accept_block {} {}", block.header().hash(), unix_time_as_millis());
            let block_hash = block.header().hash();
            let fbb = &mut FlatBufferBuilder::new();
            let message = RelayMessage::build_compact_block(fbb, block, &HashSet::new());
            fbb.finish(message, None);

            let mut known_blocks = self.peers.known_blocks.lock();
            let selected_peers: Vec<PeerIndex> = nc
                .connected_peers()
                .into_iter()
                .filter(|target_peer| {
                    known_blocks.insert(*target_peer, block_hash.clone()) && (peer != *target_peer)
                })
                .take(MAX_RELAY_PEERS)
                .collect();

            // TODO: use filter broadcast
            for target_peer in selected_peers {
                nc.send_message_to(target_peer, fbb.finished_data().to_vec());
            }
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

        let mut txs_map: FnvHashMap<ShortTransactionID, Transaction> = transactions
            .into_iter()
            .map(|tx| {
                let short_id = short_transaction_id(key0, key1, &tx.witness_hash());
                (short_id, tx)
            })
            .collect();

        {
            let tx_pool = chain_state.tx_pool();
            let iter = tx_pool.staging_txs_iter().filter_map(|entry| {
                let short_id = short_transaction_id(key0, key1, &entry.transaction.witness_hash());
                if compact_block.short_ids.contains(&short_id) {
                    Some((short_id, entry.transaction.clone()))
                } else {
                    None
                }
            });
            txs_map.extend(iter);
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

        // TODO: use filter_broadcast
        for (peer_index, txs) in peer_txs {
            let fbb = &mut FlatBufferBuilder::new();
            let message =
                RelayMessage::build_block_proposal(fbb, &txs.into_iter().collect::<Vec<_>>());
            fbb.finish(message, None);
            nc.send_message_to(peer_index, fbb.finished_data().to_vec());
        }
    }

    pub fn get_block(&self, hash: &H256) -> Option<Block> {
        self.shared.block(hash)
    }

    pub fn peers(&self) -> Arc<Peers> {
        Arc::clone(&self.peers)
    }
}

impl<CS: ChainStore> CKBProtocolHandler for Relayer<CS> {
    fn init(&mut self, nc: Box<dyn CKBProtocolContext>) {
        nc.set_notify(Duration::from_millis(100), TX_PROPOSAL_TOKEN);
    }

    fn received(
        &mut self,
        nc: Box<dyn CKBProtocolContext>,
        peer_index: PeerIndex,
        data: bytes::Bytes,
    ) {
        let msg = match get_root::<RelayMessage>(&data) {
            Ok(msg) => msg,
            _ => {
                info!(target: "sync", "Peer {} sends us a malformed message", peer_index);
                nc.ban_peer(peer_index, BAD_MESSAGE_BAN_TIME);
                return;
            }
        };

        debug!(target: "relay", "msg {:?}", msg.payload_type());
        self.process(nc.as_ref(), peer_index, msg);
    }

    fn connected(
        &mut self,
        _nc: Box<dyn CKBProtocolContext>,
        peer_index: PeerIndex,
        version: &str,
    ) {
        info!(target: "relay", "RelayProtocol({}).connected peer={}", version, peer_index);
        // do nothing
    }

    fn disconnected(&mut self, _nc: Box<dyn CKBProtocolContext>, peer_index: PeerIndex) {
        info!(target: "relay", "RelayProtocol.disconnected peer={}", peer_index);
        // TODO
    }

    fn notify(&mut self, nc: Box<dyn CKBProtocolContext>, token: u64) {
        match token {
            TX_PROPOSAL_TOKEN => self.prune_tx_proposal_request(nc.as_ref()),
            _ => unreachable!(),
        }
    }
}

pub struct RelayState {
    pub pending_compact_blocks: Mutex<FnvHashMap<H256, CompactBlock>>,
    pub inflight_proposals: Mutex<FnvHashSet<ProposalShortId>>,
    pub pending_proposals_request: Mutex<FnvHashMap<ProposalShortId, FnvHashSet<PeerIndex>>>,
    pub tx_filter: Mutex<LruCache<H256, ()>>,
}

impl Default for RelayState {
    fn default() -> Self {
        RelayState {
            pending_compact_blocks: Mutex::new(FnvHashMap::default()),
            inflight_proposals: Mutex::new(FnvHashSet::default()),
            pending_proposals_request: Mutex::new(FnvHashMap::default()),
            tx_filter: Mutex::new(LruCache::new(TX_FILTER_SIZE)),
        }
    }
}
