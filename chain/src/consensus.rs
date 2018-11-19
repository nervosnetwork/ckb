use bigint::{H256, U256};
use core::block::IndexedBlock;
use core::global::MIN_DIFFICULTY;
use core::header::{Header, RawHeader, Seal};
use core::transaction::Capacity;

pub const DEFAULT_BLOCK_REWARD: Capacity = 5_000;
pub const MAX_UNCLE_LEN: usize = 2;
pub const MAX_UNCLE_AGE: usize = 6;

#[derive(Clone, Eq, PartialEq, Debug)]
pub struct Consensus {
    pub genesis_block: IndexedBlock,
    pub min_difficulty: U256,
    pub initial_block_reward: Capacity,
}

impl Default for Consensus {
    fn default() -> Self {
        let genesis_builder = GenesisBuilder::default();
        let genesis_block = genesis_builder.build();

        Consensus {
            genesis_block,
            min_difficulty: U256::from(MIN_DIFFICULTY),
            initial_block_reward: DEFAULT_BLOCK_REWARD,
        }
    }
}

impl Consensus {
    pub fn genesis_block(&self) -> &IndexedBlock {
        &self.genesis_block
    }

    pub fn max_uncles_len(&self) -> usize {
        MAX_UNCLE_LEN
    }

    pub fn max_uncles_age(&self) -> usize {
        MAX_UNCLE_AGE
    }

    pub fn initial_block_reward(&self) -> Capacity {
        self.initial_block_reward
    }
}

#[derive(Clone, PartialEq, Eq, Default, Debug)]
pub struct GenesisBuilder {
    version: u32,
    parent_hash: H256,
    timestamp: u64,
    txs_commit: H256,
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
                difficulty: self.difficulty,
                uncles_hash: self.uncles_hash,
                cellbase_id: self.cellbase_id,
                number: 0,
            },
            seal: self.seal,
        };

        IndexedBlock {
            header: header.into(),
            transactions: vec![],
            uncles: vec![],
        }
    }
}
