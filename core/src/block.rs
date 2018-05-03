use super::{Error, ProofPublicG, ProofPublickey, PublicKey};
use super::transaction::Transaction;
use bigint::{H256, H520, U256};
use bincode::serialize;
use crypto::secp::{Privkey, Signature};
use global::MAX_TIME_DEVIAT;
use hash::sha3_256;
use merkle_root::*;
use nervos_protocol;
use proof::Proof;
use std::ops::{Deref, DerefMut};
use time::now_ms;

#[derive(Clone, Serialize, Deserialize, PartialEq, Debug)]
pub struct RawHeader {
    /// Previous hash.
    pub pre_hash: H256,
    /// Block timestamp(ms).
    pub timestamp: u64,
    /// Block height.
    pub height: u64,
    /// Transactions root.
    pub transactions_root: H256,
    /// Block difficulty.
    pub difficulty: U256,
    /// block challenge
    pub challenge: H256,
    /// Block proof
    pub proof: Proof,
}

impl RawHeader {
    pub fn cal_hash(&self) -> H256 {
        sha3_256(serialize(self).unwrap()).into()
    }
}

// FIXME: block hash not equal consensus proof hash, should be distinguished
#[derive(Clone, Serialize, Deserialize, PartialEq, Debug)]
pub struct Header {
    /// unsign header.
    pub raw: RawHeader,
    /// Block signature
    pub signature: H520,
    /// Total difficulty
    pub total_difficulty: U256,
    /// Hash
    pub hash: H256,
}

impl Deref for Header {
    type Target = RawHeader;

    fn deref(&self) -> &RawHeader {
        &self.raw
    }
}

impl DerefMut for Header {
    fn deref_mut(&mut self) -> &mut RawHeader {
        &mut self.raw
    }
}

impl Header {
    pub fn new(raw: RawHeader, total_difficulty: U256, sig: Option<H520>) -> Header {
        let hash = raw.cal_hash();
        Header {
            raw,
            hash,
            total_difficulty,
            signature: sig.unwrap_or_default(),
        }
    }

    pub fn hash(&self) -> H256 {
        self.hash
    }

    pub fn update_hash(&mut self) {
        self.hash = self.cal_hash();
    }

    /// sign header
    pub fn sign(&mut self, private_key: H256) {
        let priv_key = Privkey::from(private_key);
        let signature = priv_key.sign_recoverable(&self.hash()).unwrap().into();
        self.signature = signature;
    }

    /// recover public key
    pub fn recover_pubkey(&self) -> Result<PublicKey, Error> {
        let pubkey = Signature::from(self.signature).recover(&self.hash())?;
        Ok(*pubkey)
    }

    /// check proof
    pub fn check_difficulty(&self) -> Result<(), Error> {
        let difficulty = self.proof.difficulty();
        if difficulty > self.difficulty {
            Ok(())
        } else {
            Err(Error::InvalidDifficulty(self.difficulty, difficulty))
        }
    }

    // check time
    pub fn check_time(&self) -> Result<(), Error> {
        let now = now_ms();
        if now + MAX_TIME_DEVIAT > self.timestamp {
            Ok(())
        } else {
            Err(Error::InvalidTimestamp(self.timestamp, now))
        }
    }

    // check hash
    pub fn check_hash(&self) -> Result<(), Error> {
        let hash = self.cal_hash();
        if self.hash() == hash {
            Ok(())
        } else {
            Err(Error::InvalidHash(self.hash(), hash))
        }
    }

    // check proof
    pub fn check_proof(&self, pubkey: &ProofPublickey, g: &ProofPublicG) -> Result<(), Error> {
        if self.proof
            .verify(self.timestamp, self.height, &self.challenge, pubkey, g)
        {
            Ok(())
        } else {
            Err(Error::InvalidProof)
        }
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Debug)]
pub struct Block {
    pub header: Header,
    pub transactions: Vec<Transaction>,
}

impl Block {
    pub fn header(&self) -> &Header {
        &self.header
    }

    pub fn hash(&self) -> H256 {
        self.header.hash()
    }

    // TODO review this after POW change
    // pub fn validate(&self, kg: &KeyGroup) -> Result<(), Error> {
    pub fn validate(&self) -> Result<(), Error> {
        self.header.check_time()?;
        self.header.check_hash()?;
        self.header.check_difficulty()?;
        self.check_txs_root()?;
        // let pubkey = self.header.recover_pubkey()?;
        // let (key, g) = kg.get(&pubkey)
        //     .ok_or_else(|| Error::InvalidPublicKey(pubkey))?;
        // self.header.check_proof(&key, &g)?;
        Ok(())
    }

    pub fn check_txs_root(&self) -> Result<(), Error> {
        let txs_hash: Vec<H256> = self.transactions.iter().map(|t| t.hash()).collect();
        let txs_root = merkle_root(txs_hash.as_slice());
        if txs_root == self.header.transactions_root {
            Ok(())
        } else {
            Err(Error::InvalidTransactionsRoot(
                self.header.transactions_root,
                txs_root,
            ))
        }
    }

    pub fn sign(&mut self, private_key: H256) {
        self.header.sign(private_key);
    }

    pub fn new(
        pre_header: &Header,
        timestamp: u64,
        difficulty: U256,
        challenge: H256,
        proof: Proof,
        txs: Vec<Transaction>,
    ) -> Block {
        let txs_hash: Vec<H256> = txs.iter().map(|t| t.hash()).collect();
        let txs_root = merkle_root(txs_hash.as_slice());
        let raw = RawHeader {
            timestamp,
            difficulty,
            challenge,
            proof,
            pre_hash: pre_header.hash(),
            height: pre_header.height + 1,
            transactions_root: txs_root,
        };

        Block {
            header: Header::new(raw, pre_header.total_difficulty + difficulty, None),
            transactions: txs,
        }
    }
}

impl<'a> From<&'a nervos_protocol::Header> for Header {
    fn from(proto: &'a nervos_protocol::Header) -> Self {
        let raw = RawHeader {
            pre_hash: H256::from_slice(proto.get_parent_hash()),
            timestamp: proto.get_timestamp(),
            height: proto.get_height(),
            transactions_root: H256::from_slice(proto.get_transactions_root()),
            challenge: H256::from_slice(proto.get_challenge()),
            proof: Proof::from_slice(proto.get_proof()),
            difficulty: H256::from_slice(proto.get_difficulty()).into(),
        };

        let hash = raw.cal_hash();

        Header {
            raw,
            hash,
            signature: H520::from_slice(proto.get_signature()),
            total_difficulty: H256::from(proto.get_total_difficulty()).into(),
        }
    }
}

impl<'a> From<&'a Header> for nervos_protocol::Header {
    fn from(h: &'a Header) -> Self {
        let mut header = nervos_protocol::Header::new();
        header.set_challenge(h.challenge.to_vec());
        let temp_difficulty: H256 = h.difficulty.into();
        header.set_difficulty(temp_difficulty.to_vec());
        header.set_height(h.height);
        header.set_parent_hash(h.pre_hash.to_vec());
        header.set_proof(h.proof.sig.to_vec());
        header.set_signature(h.signature.to_vec());
        header.set_timestamp(h.timestamp);
        let temp_total_difficulty: H256 = h.total_difficulty.into();
        header.set_total_difficulty(temp_total_difficulty.to_vec());
        header.set_transactions_root(h.transactions_root.to_vec());
        header
    }
}

impl<'a> From<&'a nervos_protocol::Block> for Block {
    fn from(b: &'a nervos_protocol::Block) -> Self {
        Block {
            header: b.get_block_header().into(),
            transactions: b.get_transactions().iter().map(|t| t.into()).collect(),
        }
    }
}

impl<'a> From<&'a Block> for nervos_protocol::Block {
    fn from(b: &'a Block) -> Self {
        let mut block = nervos_protocol::Block::new();
        block.set_block_header(b.header().into());
        let transactions = b.transactions.iter().map(|t| t.into()).collect();
        block.set_transactions(transactions);
        block
    }
}
