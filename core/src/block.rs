use super::header::{Header, IndexedHeader};
use super::transaction::Transaction;
use super::Error;
use bigint::H256;
use merkle_root::*;
use nervos_protocol;

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Default, Debug)]
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

    pub fn is_genesis(&self) -> bool {
        self.header.is_genesis()
    }

    //TODO: move to verification
    pub fn validate(&self) -> Result<(), Error> {
        Ok(())
    }

    //TODO: move to verification
    pub fn check_txs_root(&self) -> Result<(), Error> {
        let txs_hash: Vec<H256> = self.transactions.iter().map(|t| t.hash()).collect();
        let txs_root = merkle_root(txs_hash.as_slice());
        if txs_root == self.header.txs_commit {
            Ok(())
        } else {
            Err(Error::InvalidTransactionsRoot(
                self.header.txs_commit,
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

#[derive(Clone, Eq, Default, Debug)]
pub struct IndexedBlock {
    pub header: IndexedHeader,
    pub transactions: Vec<Transaction>,
}

impl PartialEq for IndexedBlock {
    fn eq(&self, other: &IndexedBlock) -> bool {
        self.header == other.header
    }
}

impl IndexedBlock {
    pub fn hash(&self) -> H256 {
        self.header.hash()
    }

    pub fn header(&self) -> &IndexedHeader {
        &self.header
    }

    pub fn is_genesis(&self) -> bool {
        self.header.is_genesis()
    }
}

impl From<Block> for IndexedBlock {
    fn from(block: Block) -> Self {
        let Block {
            header,
            transactions,
        } = block;
        IndexedBlock {
            transactions,
            header: header.into(),
        }
    }
}

impl From<IndexedBlock> for Block {
    fn from(block: IndexedBlock) -> Self {
        let IndexedBlock {
            header,
            transactions,
        } = block;
        Block {
            transactions,
            header: header.header,
        }
    }
}

impl<'a> From<&'a nervos_protocol::Block> for Block {
    fn from(b: &'a nervos_protocol::Block) -> Self {
        Block {
            header: b.get_header().into(),
            transactions: b.get_transactions().iter().map(|t| t.into()).collect(),
        }
    }
}

impl<'a> From<&'a nervos_protocol::Block> for IndexedBlock {
    fn from(b: &'a nervos_protocol::Block) -> Self {
        let block: Block = b.into();
        block.into()
    }
}

impl<'a> From<&'a Block> for nervos_protocol::Block {
    fn from(b: &'a Block) -> Self {
        let mut block = nervos_protocol::Block::new();
        block.set_header(b.header().into());
        let transactions = b.transactions.iter().map(|t| t.into()).collect();
        block.set_transactions(transactions);
        block
    }
}

impl<'a> From<&'a IndexedBlock> for nervos_protocol::Block {
    fn from(b: &'a IndexedBlock) -> Self {
        let mut block = nervos_protocol::Block::new();
        block.set_header((&b.header).into());
        let transactions = b.transactions.iter().map(|t| t.into()).collect();
        block.set_transactions(transactions);
        block
    }
}
