use bigint::{H256, U256};
use bincode::serialize;
use hash::sha3_256;
use merkle_root::*;
use nervos_protocol;
use std::ops::{Deref, DerefMut};
use transaction::Transaction;

const VERSION: u32 = 0;

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Debug, Default)]
pub struct Seal {
    pub nonce: u64,
    pub mix_hash: H256,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Debug, Default)]
pub struct RawHeader {
    pub version: u32,
    //// Parent hash.
    pub parent_hash: H256,
    /// Block timestamp(ms).
    pub timestamp: u64,
    /// Genesis number is 0, Child block number is parent block number + 1.
    pub number: u64,
    /// Transactions merkle tree root.
    pub txs_commit: H256,
    /// Block difficulty.
    pub difficulty: U256,
}

impl RawHeader {
    pub fn new<'a>(
        parent_header: &Header,
        transactions: impl Iterator<Item = &'a Transaction>,
        timestamp: u64,
        difficulty: U256,
    ) -> RawHeader {
        let transactions_hash: Vec<H256> = transactions.map(|t: &Transaction| t.hash()).collect();
        let txs_commit = merkle_root(transactions_hash.as_slice());
        let parent_hash = parent_header.hash();
        let number = parent_header.number + 1;

        RawHeader {
            version: VERSION,
            parent_hash,
            txs_commit,
            timestamp,
            number,
            difficulty,
        }
    }

    pub fn pow_hash(&self) -> H256 {
        sha3_256(serialize(self).unwrap()).into()
    }

    pub fn with_seal(self, nonce: u64, mix_hash: H256) -> Header {
        Header {
            raw: self,
            seal: Seal { nonce, mix_hash },
        }
    }
}

#[derive(Clone, Serialize, Deserialize, Debug, Default, PartialEq, Eq)]
pub struct Header {
    pub raw: RawHeader,
    /// proof seal
    pub seal: Seal,
}

impl Header {
    pub fn hash(&self) -> H256 {
        sha3_256(serialize(self).unwrap()).into()
    }

    pub fn is_genesis(&self) -> bool {
        self.number == 0
    }
}

impl Deref for Header {
    type Target = RawHeader;

    fn deref(&self) -> &Self::Target {
        &self.raw
    }
}

impl DerefMut for IndexedHeader {
    fn deref_mut(&mut self) -> &mut Header {
        &mut self.header
    }
}

impl Deref for IndexedHeader {
    type Target = Header;

    fn deref(&self) -> &Self::Target {
        &self.header
    }
}

#[derive(Clone, Debug, Eq, Default)]
pub struct IndexedHeader {
    pub header: Header,
    /// memorise hash
    hash: H256,
}

impl PartialEq for IndexedHeader {
    fn eq(&self, other: &IndexedHeader) -> bool {
        self.hash == other.hash
    }
}

impl IndexedHeader {
    pub fn hash(&self) -> H256 {
        self.hash
    }

    pub fn new(header: Header, hash: H256) -> Self {
        IndexedHeader { header, hash }
    }
}

impl From<Header> for IndexedHeader {
    fn from(header: Header) -> Self {
        let hash = header.hash();
        IndexedHeader { header, hash }
    }
}

impl From<IndexedHeader> for Header {
    fn from(indexed_header: IndexedHeader) -> Self {
        indexed_header.header
    }
}

impl<'a> From<&'a nervos_protocol::Header> for Header {
    fn from(proto: &'a nervos_protocol::Header) -> Self {
        Header {
            raw: RawHeader {
                version: proto.get_version(),
                parent_hash: H256::from_slice(proto.get_parent_hash()),
                timestamp: proto.get_timestamp(),
                number: proto.get_number(),
                txs_commit: H256::from_slice(proto.get_txs_commit()),
                difficulty: H256::from_slice(proto.get_difficulty()).into(),
            },
            seal: Seal {
                nonce: proto.get_nonce(),
                mix_hash: H256::from_slice(proto.get_mix_hash()),
            },
        }
    }
}

impl<'a> From<&'a nervos_protocol::Header> for IndexedHeader {
    fn from(proto: &'a nervos_protocol::Header) -> Self {
        let header: Header = proto.into();
        header.into()
    }
}

impl<'a> From<&'a Header> for nervos_protocol::Header {
    fn from(h: &'a Header) -> Self {
        let mut header = nervos_protocol::Header::new();
        let temp_difficulty: H256 = h.difficulty.into();
        header.set_version(h.version);
        header.set_difficulty(temp_difficulty.to_vec());
        header.set_number(h.number);
        header.set_nonce(h.seal.nonce);
        header.set_mix_hash(h.seal.mix_hash.to_vec());
        header.set_parent_hash(h.parent_hash.to_vec());
        header.set_timestamp(h.timestamp);
        header.set_txs_commit(h.txs_commit.to_vec());
        header
    }
}

impl<'a> From<&'a IndexedHeader> for nervos_protocol::Header {
    fn from(h: &'a IndexedHeader) -> Self {
        let header = &h.header;
        header.into()
    }
}
