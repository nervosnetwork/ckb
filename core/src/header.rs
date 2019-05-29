use bincode::{deserialize, serialize};
use faster_hex::hex_string;
use hash::blake2b_256;
use numext_fixed_hash::H256;
use numext_fixed_uint::U256;
use serde_derive::{Deserialize, Serialize};
use std::{fmt, mem};

pub use crate::{BlockNumber, Bytes, EpochNumber, Version};

pub const HEADER_VERSION: Version = 0;

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct Seal {
    nonce: u64,
    proof: Bytes,
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
    pub fn new(nonce: u64, proof: Bytes) -> Self {
        Seal { nonce, proof }
    }

    pub fn proof(&self) -> &[u8] {
        &self.proof
    }

    pub fn destruct(self) -> (u64, Bytes) {
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
    /// Genesis number is 0, Child block number is parent block number + 1.
    number: BlockNumber,
    /// Transactions merkle root.
    transactions_root: H256,
    /// Transactions proposal hash.
    proposals_hash: H256,
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
        HeaderBuilder { raw: self, seal }.build()
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
            + mem::size_of::<u64>() * 3
            + mem::size_of::<u32>()
    }
}

#[derive(Clone, Serialize, Eq)]
pub struct Header {
    raw: RawHeader,
    /// proof seal
    seal: Seal,
    #[serde(skip)]
    hash: H256,
}

// The order of fields should be same as Header deserialization
#[derive(Deserialize)]
struct HeaderKernel {
    raw: RawHeader,
    seal: Seal,
}

// The order of fields should be same as HeaderKernel deserialization
impl<'de> serde::de::Deserialize<'de> for Header {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::de::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(field_identifier, rename_all = "snake_case")]
        enum Field {
            Raw,
            Seal,
        }

        struct InnerVisitor;

        impl<'de> serde::de::Visitor<'de> for InnerVisitor {
            type Value = Header;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("struct Header")
            }

            fn visit_seq<V>(self, mut seq: V) -> Result<Self::Value, V::Error>
            where
                V: serde::de::SeqAccess<'de>,
            {
                let raw = seq
                    .next_element()?
                    .ok_or_else(|| serde::de::Error::invalid_length(0, &self))?;
                let seal = seq
                    .next_element()?
                    .ok_or_else(|| serde::de::Error::invalid_length(1, &self))?;
                Ok(Self::Value::new(raw, seal))
            }

            fn visit_map<V>(self, mut map: V) -> Result<Self::Value, V::Error>
            where
                V: serde::de::MapAccess<'de>,
            {
                let mut raw = None;
                let mut seal = None;
                while let Some(key) = map.next_key()? {
                    match key {
                        Field::Raw => {
                            if raw.is_some() {
                                return Err(serde::de::Error::duplicate_field("raw"));
                            }
                            raw = Some(map.next_value()?);
                        }
                        Field::Seal => {
                            if seal.is_some() {
                                return Err(serde::de::Error::duplicate_field("seal"));
                            }
                            seal = Some(map.next_value()?);
                        }
                    }
                }
                let raw = raw.ok_or_else(|| serde::de::Error::missing_field("raw"))?;
                let seal = seal.ok_or_else(|| serde::de::Error::missing_field("seal"))?;
                Ok(Self::Value::new(raw, seal))
            }
        }

        const FIELDS: &[&str] = &["raw", "seal"];
        deserializer.deserialize_struct("Header", FIELDS, InnerVisitor)
    }
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
                "witnesses_root",
                &format_args!("{:#x}", self.raw.witnesses_root),
            )
            .field(
                "proposals_hash",
                &format_args!("{:#x}", self.raw.proposals_hash),
            )
            .field("difficulty", &format_args!("{:#x}", self.raw.difficulty))
            .field("uncles_count", &self.raw.uncles_count)
            .field("uncles_hash", &format_args!("{:#x}", self.raw.uncles_hash))
            .field("epoch", &self.raw.epoch)
            .field("seal", &self.seal)
            .finish()
    }
}

impl Header {
    pub(crate) fn new(raw: RawHeader, seal: Seal) -> Self {
        let mut header = Self {
            raw,
            seal,
            hash: H256::zero(),
        };
        let hash =
            blake2b_256(serialize(&header).expect("Header serialize should not fail")).into();
        header.hash = hash;
        header
    }

    /// # Warning
    ///
    /// When using this method, the caller should ensure the input hash is right, or the caller
    /// will get a incorrect Header.
    pub unsafe fn from_bytes_with_hash_unchecked(bytes: &[u8], hash: H256) -> Self {
        let HeaderKernel { raw, seal } =
            deserialize(bytes).expect("header kernel deserializing should be ok");
        Self { raw, seal, hash }
    }

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

    pub fn hash(&self) -> &H256 {
        &self.hash
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

    pub fn witnesses_root(&self) -> &H256 {
        &self.raw.witnesses_root
    }

    pub fn proposals_hash(&self) -> &H256 {
        &self.raw.proposals_hash
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
    raw: RawHeader,
    seal: Seal,
}

impl HeaderBuilder {
    pub fn from_header(header: Header) -> Self {
        let Header { raw, seal, .. } = header;
        Self { raw, seal }
    }

    pub fn seal(mut self, seal: Seal) -> Self {
        self.seal = seal;
        self
    }

    pub fn version(mut self, version: u32) -> Self {
        self.raw.version = version;
        self
    }

    pub fn number(mut self, number: BlockNumber) -> Self {
        self.raw.number = number;
        self
    }

    pub fn epoch(mut self, number: EpochNumber) -> Self {
        self.raw.epoch = number;
        self
    }

    pub fn difficulty(mut self, difficulty: U256) -> Self {
        self.raw.difficulty = difficulty;
        self
    }

    pub fn timestamp(mut self, timestamp: u64) -> Self {
        self.raw.timestamp = timestamp;
        self
    }

    pub fn proof(mut self, proof: Bytes) -> Self {
        self.seal.proof = proof;
        self
    }

    pub fn nonce(mut self, nonce: u64) -> Self {
        self.seal.nonce = nonce;
        self
    }

    pub fn parent_hash(mut self, hash: H256) -> Self {
        self.raw.parent_hash = hash;
        self
    }

    pub fn transactions_root(mut self, hash: H256) -> Self {
        self.raw.transactions_root = hash;
        self
    }

    pub fn witnesses_root(mut self, hash: H256) -> Self {
        self.raw.witnesses_root = hash;
        self
    }

    pub fn proposals_hash(mut self, hash: H256) -> Self {
        self.raw.proposals_hash = hash;
        self
    }

    pub fn uncles_hash(mut self, hash: H256) -> Self {
        self.raw.uncles_hash = hash;
        self
    }

    pub fn uncles_count(mut self, uncles_count: u32) -> Self {
        self.raw.uncles_count = uncles_count;
        self
    }

    pub fn build(self) -> Header {
        let Self { raw, seal } = self;
        Header::new(raw, seal)
    }
}
