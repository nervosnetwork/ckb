use super::header::Header;
use super::transaction::Transaction;
use super::Error;
use bigint::H256;
use merkle_root::*;
use nervos_protocol;

#[derive(Clone, Serialize, Deserialize, PartialEq, Default, Debug)]
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

    //TODO: move to verification
    pub fn validate(&self) -> Result<(), Error> {
        // self.header.check_time()?;
        // self.header.check_hash()?;
        // self.header.check_difficulty()?;
        // self.check_txs_root()?;
        // let pubkey = self.header.recover_pubkey()?;
        // let (key, g) = kg.get(&pubkey)
        //     .ok_or_else(|| Error::InvalidPublicKey(pubkey))?;
        // self.header.check_proof(&key, &g)?;
        Ok(())
    }

    //TODO: move to verification
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

    pub fn new(header: Header, transactions: Vec<Transaction>) -> Block {
        Block {
            header,
            transactions,
        }
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

#[cfg(test)]
mod tests {}
