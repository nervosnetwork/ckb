use super::header::{Header, IndexedHeader};
use super::transaction::Transaction;
use bigint::H256;
use ckb_protocol;
use uncle::{uncles_hash, UncleBlock};
use BlockNumber;

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Default, Debug)]
pub struct Block {
    pub header: Header,
    pub transactions: Vec<Transaction>,
    pub uncles: Vec<UncleBlock>,
}

impl Block {
    pub fn new(header: Header, transactions: Vec<Transaction>, uncles: Vec<UncleBlock>) -> Block {
        Block {
            header,
            transactions,
            uncles,
        }
    }

    pub fn header(&self) -> &Header {
        &self.header
    }

    pub fn number(&self) -> BlockNumber {
        self.header.number
    }

    pub fn hash(&self) -> H256 {
        self.header.hash()
    }

    pub fn is_genesis(&self) -> bool {
        self.header.is_genesis()
    }

    pub fn uncles(&self) -> &[UncleBlock] {
        &self.uncles
    }

    pub fn cal_uncles_hash(&self) -> H256 {
        uncles_hash(&self.uncles)
    }
}

#[derive(Clone, Eq, Default, Debug)]
pub struct IndexedBlock {
    pub header: IndexedHeader,
    pub transactions: Vec<Transaction>,
    pub uncles: Vec<UncleBlock>,
}

impl PartialEq for IndexedBlock {
    fn eq(&self, other: &IndexedBlock) -> bool {
        self.header == other.header
    }
}

impl ::std::hash::Hash for IndexedBlock {
    fn hash<H>(&self, state: &mut H)
    where
        H: ::std::hash::Hasher,
    {
        state.write(&self.header.hash());
        state.finish();
    }
}

impl IndexedBlock {
    pub fn hash(&self) -> H256 {
        self.header.hash()
    }

    pub fn number(&self) -> BlockNumber {
        self.header.number
    }

    pub fn header(&self) -> &IndexedHeader {
        &self.header
    }

    pub fn is_genesis(&self) -> bool {
        self.header.is_genesis()
    }

    pub fn uncles(&self) -> &Vec<UncleBlock> {
        &self.uncles
    }

    pub fn cal_uncles_hash(&self) -> H256 {
        uncles_hash(&self.uncles)
    }

    pub fn finalize_dirty(&mut self) {
        self.header.finalize_dirty()
    }
}

impl From<Block> for IndexedBlock {
    fn from(block: Block) -> Self {
        let Block {
            header,
            transactions,
            uncles,
        } = block;
        IndexedBlock {
            transactions,
            header: header.into(),
            uncles,
        }
    }
}

impl From<IndexedBlock> for Block {
    fn from(block: IndexedBlock) -> Self {
        let IndexedBlock {
            header,
            transactions,
            uncles,
        } = block;
        Block {
            transactions,
            header: header.header,
            uncles,
        }
    }
}

impl<'a> From<&'a ckb_protocol::Block> for Block {
    fn from(b: &'a ckb_protocol::Block) -> Self {
        Block {
            header: b.get_header().into(),
            transactions: b.get_transactions().iter().map(|t| t.into()).collect(),
            uncles: b.get_uncles().iter().map(|t| t.into()).collect(),
        }
    }
}

impl<'a> From<&'a ckb_protocol::Block> for IndexedBlock {
    fn from(b: &'a ckb_protocol::Block) -> Self {
        let block: Block = b.into();
        block.into()
    }
}

impl<'a> From<&'a Block> for ckb_protocol::Block {
    fn from(b: &'a Block) -> Self {
        let mut block = ckb_protocol::Block::new();
        block.set_header(b.header().into());
        let transactions = b.transactions.iter().map(|t| t.into()).collect();
        block.set_transactions(transactions);
        let uncles = b.uncles.iter().map(|t| t.into()).collect();
        block.set_uncles(uncles);
        block
    }
}

impl<'a> From<&'a IndexedBlock> for ckb_protocol::Block {
    fn from(b: &'a IndexedBlock) -> Self {
        let mut block = ckb_protocol::Block::new();
        block.set_header((&b.header).into());
        let transactions = b.transactions.iter().map(|t| t.into()).collect();
        block.set_transactions(transactions);
        let uncles = b.uncles.iter().map(|t| t.into()).collect();
        block.set_uncles(uncles);
        block
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bigint::U256;
    use header::{RawHeader, Seal};
    use protobuf;
    use protobuf::Message;

    fn dummy_block() -> IndexedBlock {
        let cellbase = dummy_cellbase();
        let uncles = vec![dummy_uncle(), dummy_uncle()];
        let header = Header {
            raw: RawHeader {
                number: 0,
                version: 0,
                parent_hash: H256::zero(),
                timestamp: 10,
                txs_commit: H256::zero(),
                difficulty: U256::zero(),
                cellbase_id: cellbase.hash(),
                uncles_hash: H256::zero(),
            },
            seal: Seal {
                nonce: 0,
                mix_hash: H256::zero(),
            },
        };

        IndexedBlock {
            header: header.into(),
            transactions: vec![cellbase],
            uncles,
        }
    }

    fn dummy_cellbase() -> Transaction {
        use transaction::{CellInput, CellOutput, VERSION};

        let inputs = vec![CellInput::new_cellbase_input(0)];
        let outputs = vec![CellOutput::new(0, vec![], H256::from(0))];
        Transaction::new(VERSION, vec![], inputs, outputs)
    }

    fn dummy_uncle() -> UncleBlock {
        let cellbase = dummy_cellbase();
        let header = Header {
            raw: RawHeader {
                number: 0,
                version: 0,
                parent_hash: H256::zero(),
                timestamp: 10,
                txs_commit: H256::zero(),
                difficulty: U256::zero(),
                cellbase_id: cellbase.hash(),
                uncles_hash: H256::zero(),
            },
            seal: Seal {
                nonce: 0,
                mix_hash: H256::zero(),
            },
        };
        UncleBlock { header, cellbase }
    }

    #[test]
    fn test_proto_convert() {
        let block = dummy_block();
        let proto_block: ckb_protocol::Block = (&block).into();
        let message = proto_block.write_to_bytes().unwrap();
        let decoded_proto_block =
            protobuf::parse_from_bytes::<ckb_protocol::Block>(&message).unwrap();
        assert_eq!(proto_block, decoded_proto_block);
        let decoded_block: IndexedBlock = (&decoded_proto_block).into();
        assert_eq!(block, decoded_block);
    }
}
