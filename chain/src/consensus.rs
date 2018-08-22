use bigint::{H256, U256};
use core::block::IndexedBlock;
use core::header::{Header, RawHeader, Seal};
use core::transaction::Capacity;
use core::BlockNumber;

pub const DEFAULT_BLOCK_REWARD: Capacity = 5_000;
pub const MAX_UNCLE_LEN: usize = 2;
pub const MAX_UNCLE_AGE: usize = 6;
pub const TRANSACTION_PROPAGATION_TIME: BlockNumber = 1;
pub const TRANSACTION_PROPAGATION_TIMEOUT: BlockNumber = 10;

//TODO：find best ORPHAN_RATE_TARGET
pub const ORPHAN_RATE_TARGET: f32 = 0.1;
pub const POW_TIME_SPAN: u64 = 12 * 60 * 60 * 1000; // 12 hours
pub const POW_SPACING: u64 = 15 * 1000; //15s

#[derive(Clone, PartialEq, Debug)]
pub struct Consensus {
    pub genesis_block: IndexedBlock,
    pub initial_block_reward: Capacity,
    pub max_uncles_age: usize,
    pub max_uncles_len: usize,
    pub orphan_rate_target: f32,
    pub pow_time_span: u64,
    pub pow_spacing: u64,
    pub transaction_propagation_time: BlockNumber,
    pub transaction_propagation_timeout: BlockNumber,
}

// genesis difficulty should not be zero
impl Default for Consensus {
    fn default() -> Self {
        let genesis_builder = GenesisBuilder::default();
        let genesis_block = genesis_builder.difficulty(U256::one()).build();

        Consensus {
            genesis_block,
            max_uncles_age: MAX_UNCLE_AGE,
            max_uncles_len: MAX_UNCLE_LEN,
            initial_block_reward: DEFAULT_BLOCK_REWARD,
            orphan_rate_target: ORPHAN_RATE_TARGET,
            pow_time_span: POW_TIME_SPAN,
            pow_spacing: POW_SPACING,
            transaction_propagation_time: TRANSACTION_PROPAGATION_TIME,
            transaction_propagation_timeout: TRANSACTION_PROPAGATION_TIMEOUT,
        }
    }
}

impl Consensus {
    pub fn set_genesis_block(mut self, genesis_block: IndexedBlock) -> Self {
        self.genesis_block = genesis_block;
        self
    }

    pub fn set_initial_block_reward(mut self, initial_block_reward: Capacity) -> Self {
        self.initial_block_reward = initial_block_reward;
        self
    }

    pub fn genesis_block(&self) -> &IndexedBlock {
        &self.genesis_block
    }

    pub fn max_uncles_len(&self) -> usize {
        self.max_uncles_len
    }

    pub fn max_uncles_age(&self) -> usize {
        self.max_uncles_age
    }

    pub fn min_difficulty(&self) -> U256 {
        self.genesis_block.header.difficulty
    }

    pub fn initial_block_reward(&self) -> Capacity {
        self.initial_block_reward
    }

    pub fn difficulty_adjustment_interval(&self) -> BlockNumber {
        self.pow_time_span / self.pow_spacing
    }

    pub fn orphan_rate_target(&self) -> f32 {
        self.orphan_rate_target
    }
}

#[derive(Clone, PartialEq, Eq, Default, Debug)]
pub struct GenesisBuilder {
    version: u32,
    parent_hash: H256,
    timestamp: u64,
    txs_commit: H256,
    txs_proposal: H256,
    difficulty: U256,
    seal: Seal,
    uncles_hash: H256,
    cellbase_id: H256,
}

impl GenesisBuilder {
    pub fn new() -> GenesisBuilder {
        GenesisBuilder::default()
    }

    pub fn version(mut self, value: u32) -> Self {
        self.version = value;
        self
    }

    pub fn parent_hash(mut self, value: H256) -> Self {
        self.parent_hash = value;
        self
    }

    pub fn timestamp(mut self, value: u64) -> Self {
        self.timestamp = value;
        self
    }

    pub fn txs_proposal(mut self, value: H256) -> Self {
        self.txs_proposal = value;
        self
    }

    pub fn txs_commit(mut self, value: H256) -> Self {
        self.txs_commit = value;
        self
    }

    pub fn difficulty(mut self, value: U256) -> Self {
        self.difficulty = value;
        self
    }

    pub fn seal(mut self, nonce: u64, mix_hash: H256) -> Self {
        self.seal = Seal { nonce, mix_hash };
        self
    }

    pub fn cellbase_id(mut self, cellbase_id: H256) -> Self {
        self.cellbase_id = cellbase_id;
        self
    }

    pub fn uncles_hash(mut self, uncles_hash: H256) -> Self {
        self.uncles_hash = uncles_hash;
        self
    }

    // verify?
    pub fn build(self) -> IndexedBlock {
        let header = Header {
            raw: RawHeader {
                version: self.version,
                parent_hash: self.parent_hash,
                timestamp: self.timestamp,
                txs_commit: self.txs_commit,
                txs_proposal: self.txs_proposal,
                difficulty: self.difficulty,
                uncles_hash: self.uncles_hash,
                cellbase_id: self.cellbase_id,
                number: 0,
            },
            seal: self.seal,
        };

        IndexedBlock {
            header: header.into(),
            uncles: vec![],
            commit_transactions: vec![],
            proposal_transactions: vec![],
        }
    }
}
