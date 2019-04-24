use bincode::{deserialize, serialize};
use faster_hex::hex_string;
use hash::blake2b_256;
use numext_fixed_hash::H256;
use numext_fixed_uint::U256;
use serde_derive::{Deserialize, Serialize};
use std::{fmt, mem};

pub use crate::{BlockNumber, EpochNumber, Version};

pub const HEADER_VERSION: Version = 0;

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct Seal {
    nonce: u64,
    #[serde(with = "serde_bytes")]
    proof: Vec<u8>,
}

impl fmt::Debug for Seal {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Seal")
            .field("nonce", &self.nonce)
            .field(
                "proof",
                &format_args!("0x{}", &hex_string(&self.proof).expect("hex proof")),
            )
            .finish()
    }
}

impl Seal {
    pub fn new(nonce: u64, proof: Vec<u8>) -> Self {
        Seal { nonce, proof }
    }

    pub fn destruct(self) -> (u64, Vec<u8>) {
        let Seal { nonce, proof } = self;
        (nonce, proof)
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Debug, Default)]
pub struct RawHeader {
    version: Version,
    /// Parent hash.
    parent_hash: H256,
    /// Block timestamp(ms).
    timestamp: u64,
    /// Index of Block in epoch
    number: BlockNumber,
    /// Transactions merkle root.
    transactions_root: H256,
    /// Transactions proposal merkle root.
    proposals_root: H256,
    /// Witness hash commitment.
    witnesses_root: H256,
    /// Block difficulty.
    difficulty: U256,
    /// Hash of the uncles
    uncles_hash: H256,
    /// Number of the uncles
    uncles_count: u32,
    /// Epoch sequence number
    epoch: EpochNumber,
}

impl RawHeader {
    pub fn pow_hash(&self) -> H256 {
        blake2b_256(serialize(self).expect("RawHeader serialize should not fail")).into()
    }

    pub fn with_seal(self, seal: Seal) -> Header {
        let builder = HeaderBuilder {
            inner: Header { raw: self, seal },
        };
        builder.build()
    }

    pub fn number(&self) -> BlockNumber {
        self.number
    }

    pub fn epoch(&self) -> EpochNumber {
        self.epoch
    }

    pub fn difficulty(&self) -> &U256 {
        &self.difficulty
    }

    pub fn uncles_count(&self) -> u32 {
        self.uncles_count
    }

    pub fn mut_uncles_count(&mut self) -> &mut u32 {
        &mut self.uncles_count
    }

    // temp
    pub const fn serialized_size() -> usize {
        mem::size_of::<Version>()
            + H256::size_of() * 5
            + U256::size_of()
            + mem::size_of::<u64>() * 2
            + mem::size_of::<u32>()
    }
}

#[derive(Clone, Serialize, Deserialize, Default, Eq)]
pub struct Header {
    raw: RawHeader,
    /// proof seal
    seal: Seal,
}

impl fmt::Debug for Header {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Header")
            .field("hash", &format_args!("{:#x}", &self.hash()))
            .field("version", &self.raw.version)
            .field("parent_hash", &format_args!("{:#x}", self.raw.parent_hash))
            .field("timestamp", &self.raw.timestamp)
            .field("number", &self.raw.number)
            .field(
                "transactions_root",
                &format_args!("{:#x}", self.raw.transactions_root),
            )
            .field(
                "proposals_root",
                &format_args!("{:#x}", self.raw.proposals_root),
            )
            .field(
                "witnesses_root",
                &format_args!("{:#x}", self.raw.witnesses_root),
            )
            .field("difficulty", &format_args!("{:#x}", self.raw.difficulty))
            .field("uncles_hash", &format_args!("{:#x}", self.raw.uncles_hash))
            .field("uncles_hash", &format_args!("{:#x}", self.raw.uncles_hash))
            .field("epoch", &self.raw.epoch)
            .field("seal", &self.seal)
            .finish()
    }
}

impl Header {
    pub fn serialized_size(proof_size: usize) -> usize {
        RawHeader::serialized_size() + proof_size + mem::size_of::<u64>()
    }

    pub fn version(&self) -> u32 {
        self.raw.version
    }

    pub fn seal(&self) -> &Seal {
        &self.seal
    }

    pub fn number(&self) -> BlockNumber {
        self.raw.number
    }

    pub fn epoch(&self) -> EpochNumber {
        self.raw.epoch
    }

    pub fn difficulty(&self) -> &U256 {
        &self.raw.difficulty
    }

    pub fn timestamp(&self) -> u64 {
        self.raw.timestamp
    }

    pub fn proof(&self) -> &[u8] {
        &self.seal.proof
    }

    pub fn nonce(&self) -> u64 {
        self.seal.nonce
    }

    pub fn hash(&self) -> H256 {
        blake2b_256(serialize(&self).expect("Header serialize should not fail")).into()
    }

    pub fn pow_hash(&self) -> H256 {
        self.raw.pow_hash()
    }

    pub fn is_genesis(&self) -> bool {
        self.number() == 0
    }

    pub fn parent_hash(&self) -> &H256 {
        &self.raw.parent_hash
    }

    pub fn transactions_root(&self) -> &H256 {
        &self.raw.transactions_root
    }

    pub fn proposals_root(&self) -> &H256 {
        &self.raw.proposals_root
    }

    pub fn witnesses_root(&self) -> &H256 {
        &self.raw.witnesses_root
    }

    pub fn uncles_hash(&self) -> &H256 {
        &self.raw.uncles_hash
    }

    pub fn raw(&self) -> &RawHeader {
        &self.raw
    }

    pub fn into_raw(self) -> RawHeader {
        self.raw
    }

    pub fn uncles_count(&self) -> u32 {
        self.raw.uncles_count
    }
}

impl PartialEq for Header {
    fn eq(&self, other: &Header) -> bool {
        self.hash() == other.hash()
    }
}

#[derive(Default)]
pub struct HeaderBuilder {
    inner: Header,
}

impl HeaderBuilder {
    pub fn new(bytes: &[u8]) -> Self {
        HeaderBuilder {
            inner: deserialize(bytes).expect("header deserializing should be ok"),
        }
    }

    pub fn header(mut self, header: Header) -> Self {
        self.inner = header;
        self
    }

    pub fn seal(mut self, seal: Seal) -> Self {
        self.inner.seal = seal;
        self
    }

    pub fn version(mut self, version: u32) -> Self {
        self.inner.raw.version = version;
        self
    }

    pub fn number(mut self, number: BlockNumber) -> Self {
        self.inner.raw.number = number;
        self
    }

    pub fn difficulty(mut self, difficulty: U256) -> Self {
        self.inner.raw.difficulty = difficulty;
        self
    }

    pub fn timestamp(mut self, timestamp: u64) -> Self {
        self.inner.raw.timestamp = timestamp;
        self
    }

    pub fn proof(mut self, proof: Vec<u8>) -> Self {
        self.inner.seal.proof = proof;
        self
    }

    pub fn nonce(mut self, nonce: u64) -> Self {
        self.inner.seal.nonce = nonce;
        self
    }

    pub fn parent_hash(mut self, hash: H256) -> Self {
        self.inner.raw.parent_hash = hash;
        self
    }

    pub fn transactions_root(mut self, hash: H256) -> Self {
        self.inner.raw.transactions_root = hash;
        self
    }

    pub fn proposals_root(mut self, hash: H256) -> Self {
        self.inner.raw.proposals_root = hash;
        self
    }

    pub fn witnesses_root(mut self, hash: H256) -> Self {
        self.inner.raw.witnesses_root = hash;
        self
    }

    pub fn uncles_hash(mut self, hash: H256) -> Self {
        self.inner.raw.uncles_hash = hash;
        self
    }

    pub fn uncles_count(mut self, uncles_count: u32) -> Self {
        self.inner.raw.uncles_count = uncles_count;
        self
    }

    pub fn build(self) -> Header {
        self.inner
    }
}
