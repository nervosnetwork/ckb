#![allow(dead_code)]
#![allow(missing_docs)]

use crate::mock_utils::{create_cellbase, dao_data};
use crate::MockStore;
use ckb_chain_spec::consensus::Consensus;
use ckb_store::ChainStore;
use ckb_types::core::{BlockBuilder, BlockView, HeaderView, TransactionView};
use ckb_types::utilities::difficulty_to_compact;
use ckb_types::{packed, U256};

#[derive(Clone)]
pub struct MockChain<'a> {
    blocks: Vec<BlockView>,
    parent: HeaderView,
    consensus: &'a Consensus,
}

impl<'a> MockChain<'a> {
    pub fn new(parent: HeaderView, consensus: &'a Consensus) -> Self {
        Self {
            blocks: vec![],
            parent,
            consensus,
        }
    }

    fn commit_block(&mut self, store: &MockStore, block: BlockView) {
        store.insert_block(&block, self.consensus.genesis_epoch_ext());
        self.blocks.push(block);
    }

    pub fn rollback(&mut self, store: &MockStore) {
        if let Some(block) = self.blocks.pop() {
            store.remove_block(&block);
        }
    }

    pub fn gen_block_with_proposal_txs(&mut self, txs: Vec<TransactionView>, store: &MockStore) {
        let parent = self.tip_header();
        let cellbase = create_cellbase(store, self.consensus, &parent);
        let dao = dao_data(self.consensus, &parent, &[cellbase.clone()], store, false);

        let epoch = self
            .consensus
            .next_epoch_ext(&parent, &store.store().borrow_as_data_loader())
            .unwrap()
            .epoch();

        let new_block = BlockBuilder::default()
            .parent_hash(parent.hash())
            .number(parent.number() + 1)
            .compact_target(epoch.compact_target())
            .epoch(epoch.number_with_fraction(parent.number() + 1))
            .dao(dao)
            .transaction(cellbase)
            .proposals(txs.iter().map(TransactionView::proposal_short_id))
            .build();

        self.commit_block(store, new_block)
    }

    pub fn gen_block_with_proposal_ids(
        &mut self,
        difficulty: u64,
        ids: Vec<packed::ProposalShortId>,
        store: &MockStore,
    ) {
        let parent = self.tip_header();
        let cellbase = create_cellbase(store, self.consensus, &parent);
        let dao = dao_data(self.consensus, &parent, &[cellbase.clone()], store, false);

        let epoch = self
            .consensus
            .next_epoch_ext(&parent, &store.store().borrow_as_data_loader())
            .unwrap()
            .epoch();

        let new_block = BlockBuilder::default()
            .parent_hash(parent.hash())
            .number(parent.number() + 1)
            .epoch(epoch.number_with_fraction(parent.number() + 1))
            .compact_target(difficulty_to_compact(U256::from(difficulty)))
            .dao(dao)
            .transaction(cellbase)
            .proposals(ids)
            .build();

        self.commit_block(store, new_block)
    }

    pub fn gen_empty_block_with_diff(&mut self, difficulty: u64, store: &MockStore) {
        let parent = self.tip_header();
        let cellbase = create_cellbase(store, self.consensus, &parent);
        let dao = dao_data(self.consensus, &parent, &[cellbase.clone()], store, false);

        let epoch = self
            .consensus
            .next_epoch_ext(&parent, &store.store().borrow_as_data_loader())
            .unwrap()
            .epoch();

        let new_block = BlockBuilder::default()
            .parent_hash(parent.hash())
            .number(parent.number() + 1)
            .epoch(epoch.number_with_fraction(parent.number() + 1))
            .compact_target(difficulty_to_compact(U256::from(difficulty)))
            .dao(dao)
            .transaction(cellbase)
            .build();

        self.commit_block(store, new_block)
    }

    pub fn gen_empty_block_with_inc_diff(&mut self, inc: u64, store: &MockStore) {
        let difficulty = self.difficulty();
        let parent = self.tip_header();
        let cellbase = create_cellbase(store, self.consensus, &parent);
        let dao = dao_data(self.consensus, &parent, &[cellbase.clone()], store, false);

        let epoch = self
            .consensus
            .next_epoch_ext(&parent, &store.store().borrow_as_data_loader())
            .unwrap()
            .epoch();

        let new_block = BlockBuilder::default()
            .parent_hash(parent.hash())
            .number(parent.number() + 1)
            .compact_target(difficulty_to_compact(difficulty + U256::from(inc)))
            .epoch(epoch.number_with_fraction(parent.number() + 1))
            .dao(dao)
            .transaction(cellbase)
            .build();

        self.commit_block(store, new_block)
    }

    pub fn gen_empty_block_with_nonce(&mut self, nonce: u128, store: &MockStore) {
        let parent = self.tip_header();
        let cellbase = create_cellbase(store, self.consensus, &parent);
        let dao = dao_data(self.consensus, &parent, &[cellbase.clone()], store, false);

        let epoch = self
            .consensus
            .next_epoch_ext(&parent, &store.store().borrow_as_data_loader())
            .unwrap()
            .epoch();

        let new_block = BlockBuilder::default()
            .parent_hash(parent.hash())
            .number(parent.number() + 1)
            .compact_target(epoch.compact_target())
            .epoch(epoch.number_with_fraction(parent.number() + 1))
            .nonce(nonce)
            .dao(dao)
            .transaction(cellbase)
            .build();

        self.commit_block(store, new_block)
    }

    pub fn gen_empty_block(&mut self, store: &MockStore) {
        let parent = self.tip_header();
        let cellbase = create_cellbase(store, self.consensus, &parent);
        let dao = dao_data(self.consensus, &parent, &[cellbase.clone()], store, false);

        let epoch = self
            .consensus
            .next_epoch_ext(&parent, &store.store().borrow_as_data_loader())
            .unwrap()
            .epoch();

        let new_block = BlockBuilder::default()
            .parent_hash(parent.hash())
            .number(parent.number() + 1)
            .compact_target(epoch.compact_target())
            .epoch(epoch.number_with_fraction(parent.number() + 1))
            .dao(dao)
            .transaction(cellbase)
            .build();

        self.commit_block(store, new_block)
    }

    pub fn gen_block_with_commit_txs(
        &mut self,
        txs: Vec<TransactionView>,
        store: &MockStore,
        ignore_resolve_error: bool,
    ) {
        let parent = self.tip_header();
        let cellbase = create_cellbase(store, self.consensus, &parent);
        let mut txs_to_resolve = vec![cellbase.clone()];
        txs_to_resolve.extend_from_slice(&txs);
        let dao = dao_data(
            self.consensus,
            &parent,
            &txs_to_resolve,
            store,
            ignore_resolve_error,
        );

        let epoch = self
            .consensus
            .next_epoch_ext(&parent, &store.store().borrow_as_data_loader())
            .unwrap()
            .epoch();

        let new_block = BlockBuilder::default()
            .parent_hash(parent.hash())
            .number(parent.number() + 1)
            .compact_target(epoch.compact_target())
            .epoch(epoch.number_with_fraction(parent.number() + 1))
            .dao(dao)
            .transaction(cellbase)
            .transactions(txs)
            .build();

        self.commit_block(store, new_block)
    }

    pub fn tip_header(&self) -> HeaderView {
        self.blocks
            .last()
            .map_or(self.parent.clone(), |b| b.header())
    }

    pub fn tip(&self) -> &BlockView {
        self.blocks.last().expect("should have tip")
    }

    pub fn difficulty(&self) -> U256 {
        self.tip_header().difficulty()
    }

    pub fn blocks(&self) -> &Vec<BlockView> {
        &self.blocks
    }

    pub fn total_difficulty(&self) -> U256 {
        self.blocks()
            .iter()
            .fold(U256::from(0u64), |sum, b| sum + b.header().difficulty())
    }
}
