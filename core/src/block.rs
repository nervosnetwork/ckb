use super::{Error, PublicKey};
use super::transaction::Transaction;
use bigint::{H256, H520, U256};
use bincode::serialize;
use crypto::secp::{Privkey, Signature};
use global::MAX_TIME_DEVIAT;
use hash::sha3_256;
use keygroup::KeyGroup;
use merkle_root::*;
use proof::Proof;
use std::ops::{Deref, DerefMut};
use time::now_ms;

#[derive(Clone, Serialize, Deserialize, PartialEq, Debug)]
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

#[derive(Clone, Serialize, Deserialize, PartialEq, Debug)]
pub struct Header {
    /// unsign header.
    pub unsign_header: UnsignHeader,
    /// Block signature
    pub signature: H520,
    /// Total difficulty
    pub total_difficulty: U256,
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
    pub fn new(unsign_header: UnsignHeader, total_difficulty: U256) -> Header {
        Header {
            unsign_header: unsign_header,
            signature: H520::default(),
            total_difficulty: total_difficulty,
        }
    }
    pub fn hash(&self) -> H256 {
        sha3_256(serialize(&self.unsign_header).unwrap()).into()
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

    // check proof
    pub fn check_proof(&self, pubkey: Vec<u8>, g: Vec<u8>) -> Result<(), Error> {
        if self.proof
            .verify(self.timestamp, self.height, self.challenge, pubkey, g)
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

impl Deref for Block {
    type Target = Header;

    fn deref(&self) -> &Header {
        &self.header
    }
}

impl DerefMut for Block {
    fn deref_mut(&mut self) -> &mut Header {
        &mut self.header
    }
}

impl Block {
    pub fn validate(&self, kg: &KeyGroup) -> Result<(), Error> {
        self.check_time()?;
        self.check_difficulty()?;
        self.check_txs_root()?;
        let pubkey = self.recover_pubkey()?;
        let (key, g) = kg.get(&pubkey)
            .ok_or_else(|| Error::InvalidPublicKey(pubkey))?;
        self.check_proof(key, g)?;
        Ok(())
    }

    pub fn check_txs_root(&self) -> Result<(), Error> {
        let txs_hash: Vec<H256> = self.transactions.iter().map(|t| t.hash()).collect();
        let txs_root = merkle_root(txs_hash.as_slice());
        if txs_root == self.transactions_root {
            Ok(())
        } else {
            Err(Error::InvalidTransactionsRoot(
                self.transactions_root,
                txs_root,
            ))
        }
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
        let unsign_header = UnsignHeader {
            pre_hash: pre_header.hash(),
            timestamp: timestamp,
            height: pre_header.height + 1,
            transactions_root: txs_root,
            difficulty: difficulty,
            challenge: challenge,
            proof: proof,
        };

        Block {
            header: Header::new(unsign_header, pre_header.total_difficulty + difficulty),
            transactions: txs,
        }
    }
}
