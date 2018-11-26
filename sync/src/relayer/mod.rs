#![cfg_attr(feature = "cargo-clippy", allow(needless_pass_by_value))]

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
use bigint::H256;
use ckb_chain::chain::ChainController;
use ckb_core::block::{Block, BlockBuilder};
use ckb_core::transaction::{ProposalShortId, Transaction};
use ckb_network::{CKBProtocolContext, CKBProtocolHandler, PeerIndex, TimerToken};
use ckb_pool::txs_pool::TransactionPoolController;
use ckb_protocol::{short_transaction_id, short_transaction_id_keys, RelayMessage, RelayPayload};
use ckb_shared::index::ChainIndex;
use ckb_shared::shared::{ChainProvider, Shared};
use ckb_util::{Mutex, RwLock};
use flatbuffers::{get_root, FlatBufferBuilder};
use fnv::{FnvHashMap, FnvHashSet};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

pub const TX_PROPOSAL_TOKEN: TimerToken = 0;

pub struct Relayer<CI: ChainIndex> {
    chain: ChainController,
    shared: Shared<CI>,
    tx_pool: TransactionPoolController,
    state: Arc<RelayState>,
}

impl<CI: ChainIndex> ::std::clone::Clone for Relayer<CI> {
    fn clone(&self) -> Self {
        Relayer {
            chain: self.chain.clone(),
            shared: self.shared.clone(),
            tx_pool: self.tx_pool.clone(),
            state: Arc::clone(&self.state),
        }
    }
}

impl<CI> Relayer<CI>
where
    CI: ChainIndex + 'static,
{
    pub fn new(
        chain: ChainController,
        shared: Shared<CI>,
        tx_pool: TransactionPoolController,
    ) -> Self {
        Relayer {
            chain,
            shared,
            tx_pool,
            state: Arc::new(RelayState::default()),
        }
    }

    fn process(&self, nc: &CKBProtocolContext, peer: PeerIndex, message: RelayMessage) {
        match message.payload_type() {
            RelayPayload::CompactBlock => CompactBlockProcess::new(
                &message.payload_as_compact_block().unwrap(),
                self,
                peer,
                nc,
            ).execute(),
            RelayPayload::Transaction => {
                TransactionProcess::new(&message.payload_as_transaction().unwrap(), self, peer, nc)
                    .execute()
            }
            RelayPayload::GetBlockTransactions => GetBlockTransactionsProcess::new(
                &message.payload_as_get_block_transactions().unwrap(),
                self,
                peer,
                nc,
            ).execute(),
            RelayPayload::BlockTransactions => BlockTransactionsProcess::new(
                &message.payload_as_block_transactions().unwrap(),
                self,
                peer,
                nc,
            ).execute(),
            RelayPayload::GetBlockProposal => GetBlockProposalProcess::new(
                &message.payload_as_get_block_proposal().unwrap(),
                self,
                peer,
                nc,
            ).execute(),
            RelayPayload::BlockProposal => {
                BlockProposalProcess::new(&message.payload_as_block_proposal().unwrap(), self)
                    .execute()
            }
            RelayPayload::NONE => {}
        }
    }

    pub fn request_proposal_txs(
        &self,
        nc: &CKBProtocolContext,
        peer: PeerIndex,
        block: &CompactBlock,
    ) {
        let mut inflight = self.state.inflight_proposals.lock();
        let unknown_ids = block
            .proposal_transactions
            .iter()
            .chain(
                block
                    .uncles
                    .iter()
                    .flat_map(|uncle| uncle.proposal_transactions()),
            ).filter(|x| !self.tx_pool.contains_key(**x) && inflight.insert(**x))
            .cloned()
            .collect::<Vec<_>>();

        let fbb = &mut FlatBufferBuilder::new();
        let message =
            RelayMessage::build_get_block_proposal(fbb, block.header.number(), &unknown_ids);
        fbb.finish(message, None);

        let _ = nc.send(peer, fbb.finished_data().to_vec());
    }

    pub fn accept_block(&self, nc: &CKBProtocolContext, peer: PeerIndex, block: &Arc<Block>) {
        if self.chain.process_block(Arc::clone(&block)).is_ok() {
            let fbb = &mut FlatBufferBuilder::new();
            let message = RelayMessage::build_compact_block(fbb, block, &HashSet::new());
            fbb.finish(message, None);

            for peer_id in nc.connected_peers() {
                if peer_id != peer {
                    let _ = nc.send(peer_id, fbb.finished_data().to_vec());
                }
            }
        }
    }

    pub fn reconstruct_block(
        &self,
        compact_block: &CompactBlock,
        transactions: Vec<Transaction>,
    ) -> (Option<Block>, Vec<usize>) {
        let (key0, key1) =
            short_transaction_id_keys(compact_block.header.nonce(), compact_block.nonce);

        let mut txs = transactions;
        txs.extend(self.tx_pool.get_potential_transactions());

        let mut txs_map = FnvHashMap::default();
        for tx in txs {
            let short_id = short_transaction_id(key0, key1, &tx.hash());
            txs_map.insert(short_id, tx);
        }

        let short_ids_iter = &mut compact_block.short_ids.iter();
        let mut block_transactions = Vec::with_capacity(
            compact_block.prefilled_transactions.len() + compact_block.short_ids.len(),
        );

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

        let mut missing_indexes = Vec::new();
        for (i, t) in block_transactions.iter().enumerate() {
            if t.is_none() {
                missing_indexes.push(i);
            }
        }

        if missing_indexes.is_empty() {
            let block = BlockBuilder::default()
                .header(compact_block.header.clone())
                .uncles(compact_block.uncles.clone())
                .commit_transactions(block_transactions.into_iter().map(|t| t.unwrap()).collect())
                .proposal_transactions(compact_block.proposal_transactions.clone())
                .build();

            (Some(block), missing_indexes)
        } else {
            (None, missing_indexes)
        }
    }

    fn prune_tx_proposal_request(&self, nc: &CKBProtocolContext) {
        let mut pending_proposals_request = self.state.pending_proposals_request.lock();
        let mut peer_txs = FnvHashMap::default();
        let mut remove_ids = Vec::new();

        for (id, peers) in pending_proposals_request.iter() {
            if let Some(tx) = self.tx_pool.get_transaction(*id) {
                for peer in peers {
                    let mut tx_set = peer_txs.entry(*peer).or_insert_with(Vec::new);
                    tx_set.push(tx.clone());
                }
            }
            remove_ids.push(*id);
        }

        for id in remove_ids {
            pending_proposals_request.remove(&id);
        }

        for (peer, txs) in peer_txs {
            let fbb = &mut FlatBufferBuilder::new();
            let message = RelayMessage::build_block_proposal(
                fbb,
                &txs.into_iter().map(Into::into).collect::<Vec<_>>(),
            );
            fbb.finish(message, None);

            let _ = nc.send(peer, fbb.finished_data().to_vec());
        }
    }

    pub fn get_block(&self, hash: &H256) -> Option<Block> {
        self.shared.block(hash)
    }
}

impl<CI> CKBProtocolHandler for Relayer<CI>
where
    CI: ChainIndex + 'static,
{
    fn initialize(&self, nc: Box<CKBProtocolContext>) {
        let _ = nc.register_timer(TX_PROPOSAL_TOKEN, Duration::from_millis(100));
    }

    fn received(&self, nc: Box<CKBProtocolContext>, peer: PeerIndex, data: &[u8]) {
        // TODO use flatbuffers verifier
        let msg = get_root::<RelayMessage>(data);
        debug!(target: "relay", "msg {:?}", msg.payload_type());
        self.process(nc.as_ref(), peer, msg);
    }

    fn connected(&self, _nc: Box<CKBProtocolContext>, peer: PeerIndex) {
        info!(target: "sync", "peer={} RelayProtocol.connected", peer);
        // do nothing
    }

    fn disconnected(&self, _nc: Box<CKBProtocolContext>, peer: PeerIndex) {
        info!(target: "sync", "peer={} RelayProtocol.disconnected", peer);
        // TODO
    }

    fn timer_triggered(&self, nc: Box<CKBProtocolContext>, token: TimerToken) {
        match token as usize {
            TX_PROPOSAL_TOKEN => self.prune_tx_proposal_request(nc.as_ref()),
            _ => unreachable!(),
        }
    }
}

#[derive(Default)]
pub struct RelayState {
    pub pending_compact_blocks: RwLock<FnvHashMap<H256, CompactBlock>>,
    pub inflight_proposals: Mutex<FnvHashSet<ProposalShortId>>,
    pub pending_proposals_request: Mutex<FnvHashMap<ProposalShortId, FnvHashSet<PeerIndex>>>,
}
