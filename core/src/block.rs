use super::transaction::Transaction;
use bigint::{H256, H520, U256};
use bincode::serialize;
use crypto::secp::Privkey;
use hash::sha3_256;
use merkle_root::*;
use proof::Proof;
use std::ops::{Deref, DerefMut};

#[derive(Serialize, Deserialize, PartialEq, Debug)]
pub struct UnsignHeader {
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

#[derive(Serialize, Deserialize, PartialEq, Debug)]
pub struct Header {
    /// unsign header.
    pub unsign_header: UnsignHeader,
    /// Block signature
    pub signature: H520,
}

impl Deref for Header {
    type Target = UnsignHeader;

    fn deref(&self) -> &UnsignHeader {
        &self.unsign_header
    }
}

impl DerefMut for Header {
    fn deref_mut(&mut self) -> &mut UnsignHeader {
        &mut self.unsign_header
    }
}

impl Header {
    pub fn new(unsign_header: UnsignHeader) -> Header {
        Header {
            unsign_header: unsign_header,
            signature: H520::default(),
        }
    }
    pub fn hash(&self) -> H256 {
        sha3_256(serialize(&self.unsign_header).unwrap()).into()
    }

    ///sign header
    pub fn sign(&mut self, private_key: H256) {
        let priv_key = Privkey::from(private_key);
        let signature = priv_key.sign_recoverable(&self.hash()).unwrap().into();
        self.signature = signature;
    }
}

#[derive(Serialize, Deserialize, PartialEq, Debug)]
pub struct Block {
    pub header: Header,
    pub transactions: Vec<Transaction>,
}

impl Block {
    pub fn validate(&self) -> bool {
        true
    }

    pub fn hash(&self) -> H256 {
        self.header.hash()
    }

    pub fn sign(&mut self, private_key: H256) {
        self.header.sign(private_key);
    }

    pub fn new(
        pre_hash: H256,
        timestamp: u64,
        height: u64,
        difficulty: U256,
        challenge: H256,
        proof: Proof,
        txs: Vec<Transaction>,
    ) -> Block {
        let txs_hash: Vec<H256> = txs.iter().map(|t| t.hash()).collect();
        let txs_root = merkle_root(txs_hash.as_slice());
        let unsign_header = UnsignHeader {
            pre_hash: pre_hash,
            timestamp: timestamp,
            height: height,
            transactions_root: txs_root,
            difficulty: difficulty,
            challenge: challenge,
            proof: proof,
        };

        Block {
            header: Header::new(unsign_header),
            transactions: txs,
        }
    }
}
