use crate::consensus::Consensus;
use crate::consensus::{
    build_genesis_epoch_ext, ConsensusBuilder, DEFAULT_EPOCH_DURATION_TARGET,
    DEFAULT_ORPHAN_RATE_TARGET,
};
use crate::versionbits::{
    ActiveMode, Deployment, DeploymentPos, ThresholdState, VersionbitsIndexer,
};
use crate::TESTNET_ACTIVATION_THRESHOLD;
use ckb_types::{
    core::{
        capacity_bytes, BlockBuilder, BlockView, Capacity, EpochExt, HeaderView,
        TransactionBuilder, TransactionView, Version,
    },
    packed::{Byte32, Bytes, CellbaseWitness},
    prelude::*,
    utilities::DIFF_TWO,
};
use std::collections::HashMap;

type Index = Byte32;
type BlockHash = Byte32;

#[derive(Clone, Debug)]
struct MockChain {
    consensus: Consensus,
    current_epoch_ext: EpochExt,
    tip: HeaderView,
    cellbases: HashMap<BlockHash, TransactionView>,
    headers: HashMap<BlockHash, HeaderView>,
    epoch_index: HashMap<BlockHash, Index>,
    epoch_exts: HashMap<Index, EpochExt>,
}

impl VersionbitsIndexer for MockChain {
    fn block_epoch_index(&self, block_hash: &Byte32) -> Option<Byte32> {
        self.epoch_index.get(block_hash).cloned()
    }

    fn epoch_ext(&self, index: &Byte32) -> Option<EpochExt> {
        self.epoch_exts.get(index).cloned()
    }

    fn block_header(&self, block_hash: &Byte32) -> Option<HeaderView> {
        self.headers.get(block_hash).cloned()
    }

    fn cellbase(&self, block_hash: &Byte32) -> Option<TransactionView> {
        self.cellbases.get(block_hash).cloned()
    }
}

impl MockChain {
    fn new(consensus: Consensus) -> Self {
        let genesis = consensus.genesis_block();
        let genesis_epoch_ext = consensus.genesis_epoch_ext.clone();
        let index = genesis_epoch_ext.last_block_hash_in_previous_epoch();

        let mut cellbases = HashMap::new();
        let mut headers = HashMap::new();
        let mut epoch_index = HashMap::new();
        let mut epoch_exts = HashMap::new();

        let tip = genesis.header();

        cellbases.insert(genesis.hash(), genesis.transactions()[0].clone());
        headers.insert(genesis.hash(), genesis.header());
        epoch_index.insert(genesis.hash(), index.clone());
        epoch_exts.insert(index, genesis_epoch_ext.clone());

        MockChain {
            consensus,
            cellbases,
            headers,
            epoch_index,
            epoch_exts,
            tip,
            current_epoch_ext: genesis_epoch_ext,
        }
    }

    fn compute_versionbits(&self, parent: &HeaderView) -> Option<Version> {
        self.consensus.compute_versionbits(parent, self)
    }

    fn get_state(&self, pos: DeploymentPos) -> Option<ThresholdState> {
        self.consensus.versionbits_state(pos, &self.tip, self)
    }

    fn advanced_next_epoch(&mut self) {
        let index = self.tip.epoch().index();
        let length = self.tip.epoch().length();

        let remain = length - index - 1;

        for _ in 0..remain {
            let block = self.next_signal_block();
            self.insert_block(block);
        }

        let next_epoch = self.next_epoch(&self.tip);
        self.insert_epoch_ext(next_epoch);

        let block = self.next_signal_block();
        self.insert_block(block);
    }

    fn advanced_next_epoch_without_signal(&mut self) {
        let index = self.tip.epoch().index();
        let length = self.tip.epoch().length();

        let remain = length - index - 1;
        for _ in 0..remain {
            let block = self.next_block();
            self.insert_block(block);
        }

        let next_epoch = self.next_epoch(&self.tip);
        self.insert_epoch_ext(next_epoch);

        let block = self.next_signal_block();
        self.insert_block(block);
    }

    fn insert_epoch_ext(&mut self, epoch: EpochExt) {
        let index = epoch.last_block_hash_in_previous_epoch();
        self.epoch_exts.insert(index, epoch.clone());
        self.current_epoch_ext = epoch;
    }

    fn insert_block(&mut self, block: BlockView) {
        let index = self.current_epoch_ext.last_block_hash_in_previous_epoch();
        let new_tip = block.header();
        self.cellbases
            .insert(block.hash(), block.transactions()[0].clone());
        self.headers.insert(block.hash(), block.header());
        self.epoch_index.insert(block.hash(), index);
        self.tip = new_tip;
    }

    fn next_epoch(&self, last_header: &HeaderView) -> EpochExt {
        let current_epoch_number = self.current_epoch_ext.number();
        self.current_epoch_ext
            .clone()
            .into_builder()
            .number(current_epoch_number + 1)
            .last_block_hash_in_previous_epoch(last_header.hash())
            .start_number(last_header.number() + 1)
            .build()
    }

    fn next_block(&self) -> BlockView {
        let parent = &self.tip;
        let epoch = &self.current_epoch_ext;

        let cellbase = TransactionBuilder::default().build();
        BlockBuilder::default()
            .parent_hash(parent.hash())
            .number((parent.number() + 1).pack())
            .epoch(epoch.number_with_fraction(parent.number() + 1).pack())
            .transaction(cellbase)
            .build()
    }

    fn next_signal_block(&self) -> BlockView {
        let parent = &self.tip;
        let epoch = &self.current_epoch_ext;

        let version = self.compute_versionbits(parent).unwrap();

        let cellbase_witness = CellbaseWitness::new_builder()
            .message(version.to_le_bytes().as_slice().pack())
            .build();

        let witness = cellbase_witness.as_bytes().pack();
        let cellbase = TransactionBuilder::default().witness(witness).build();

        BlockBuilder::default()
            .parent_hash(parent.hash())
            .number((parent.number() + 1).pack())
            .epoch(epoch.number_with_fraction(parent.number() + 1).pack())
            .transaction(cellbase)
            .build()
    }
}

#[test]
fn test_versionbits_active() {
    let cellbase = TransactionBuilder::default()
        .witness(Bytes::default())
        .build();
    let epoch_ext = build_genesis_epoch_ext(
        capacity_bytes!(100),
        DIFF_TWO,
        4,
        DEFAULT_EPOCH_DURATION_TARGET,
        DEFAULT_ORPHAN_RATE_TARGET,
    );
    let genesis = BlockBuilder::default()
        .epoch(epoch_ext.number_with_fraction(0).pack())
        .transaction(cellbase)
        .build();

    let mut deployments = HashMap::new();
    let test_dummy = Deployment {
        bit: 1,
        start: 1,
        timeout: 11,
        min_activation_epoch: 11,
        period: 2,
        active_mode: ActiveMode::Normal,
        threshold: TESTNET_ACTIVATION_THRESHOLD,
    };
    deployments.insert(DeploymentPos::Testdummy, test_dummy);

    let consensus = ConsensusBuilder::new(genesis, epoch_ext)
        .softfork_deployments(deployments)
        .build();
    let mut chain = MockChain::new(consensus);

    assert_eq!(chain.current_epoch_ext.number(), 0);
    assert_eq!(
        chain.get_state(DeploymentPos::Testdummy),
        Some(ThresholdState::Defined)
    );

    chain.advanced_next_epoch();
    assert_eq!(chain.current_epoch_ext.number(), 1);
    assert_eq!(
        chain.get_state(DeploymentPos::Testdummy),
        Some(ThresholdState::Started)
    );

    chain.advanced_next_epoch();
    assert_eq!(chain.current_epoch_ext.number(), 2);
    assert_eq!(
        chain.get_state(DeploymentPos::Testdummy),
        Some(ThresholdState::Started)
    );

    for _ in 0..8 {
        chain.advanced_next_epoch();
        assert_eq!(
            chain.get_state(DeploymentPos::Testdummy),
            Some(ThresholdState::LockedIn)
        );
    }

    chain.advanced_next_epoch();
    assert_eq!(chain.current_epoch_ext.number(), 11);
    assert_eq!(
        chain.get_state(DeploymentPos::Testdummy),
        Some(ThresholdState::Active)
    );

    chain.advanced_next_epoch();
    assert_eq!(chain.current_epoch_ext.number(), 12);
    assert_eq!(
        chain.get_state(DeploymentPos::Testdummy),
        Some(ThresholdState::Active)
    );
}

#[test]
fn test_versionbits_failed() {
    let cellbase = TransactionBuilder::default()
        .witness(Bytes::default())
        .build();
    let epoch_ext = build_genesis_epoch_ext(
        capacity_bytes!(100),
        DIFF_TWO,
        4,
        DEFAULT_EPOCH_DURATION_TARGET,
        DEFAULT_ORPHAN_RATE_TARGET,
    );
    let genesis = BlockBuilder::default()
        .epoch(epoch_ext.number_with_fraction(0).pack())
        .transaction(cellbase)
        .build();

    let mut deployments = HashMap::new();
    let test_dummy = Deployment {
        bit: 1,
        start: 1,
        timeout: 11,
        min_activation_epoch: 11,
        period: 2,
        active_mode: ActiveMode::Normal,
        threshold: TESTNET_ACTIVATION_THRESHOLD,
    };
    deployments.insert(DeploymentPos::Testdummy, test_dummy);

    let consensus = ConsensusBuilder::new(genesis, epoch_ext)
        .softfork_deployments(deployments)
        .build();
    let mut chain = MockChain::new(consensus);

    assert_eq!(chain.current_epoch_ext.number(), 0);
    assert_eq!(
        chain.get_state(DeploymentPos::Testdummy),
        Some(ThresholdState::Defined)
    );

    for _ in 0..10 {
        chain.advanced_next_epoch_without_signal();
        assert_eq!(
            chain.get_state(DeploymentPos::Testdummy),
            Some(ThresholdState::Started)
        );
    }

    chain.advanced_next_epoch_without_signal();
    assert_eq!(chain.current_epoch_ext.number(), 11);
    assert_eq!(
        chain.get_state(DeploymentPos::Testdummy),
        Some(ThresholdState::Failed)
    );

    chain.advanced_next_epoch_without_signal();
    assert_eq!(chain.current_epoch_ext.number(), 12);
    assert_eq!(
        chain.get_state(DeploymentPos::Testdummy),
        Some(ThresholdState::Failed)
    );
}
