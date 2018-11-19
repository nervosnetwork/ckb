use super::header::{Header, IndexedHeader};
use super::transaction::{IndexedTransaction, ProposalShortId, Transaction};
use bigint::H256;
use ckb_protocol;
use rayon::iter::{IntoParallelIterator, ParallelIterator};
use uncle::{uncles_hash, UncleBlock};
use BlockNumber;

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Default, Debug)]
pub struct Block {
    pub header: Header,
    pub uncles: Vec<UncleBlock>,
    pub commit_transactions: Vec<Transaction>,
    pub proposal_transactions: Vec<ProposalShortId>,
}

impl Block {
    pub fn new(
        header: Header,
        uncles: Vec<UncleBlock>,
        commit_transactions: Vec<Transaction>,
        proposal_transactions: Vec<ProposalShortId>,
    ) -> Block {
        Block {
            header,
            uncles,
            commit_transactions,
            proposal_transactions,
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

    pub fn commit_transactions(&self) -> &[Transaction] {
        &self.commit_transactions
    }

    pub fn proposal_transactions(&self) -> &[ProposalShortId] {
        &self.proposal_transactions
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
    pub uncles: Vec<UncleBlock>,
    pub commit_transactions: Vec<IndexedTransaction>,
    pub proposal_transactions: Vec<ProposalShortId>,
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
    pub fn new(
        header: IndexedHeader,
        uncles: Vec<UncleBlock>,
        commit_transactions: Vec<IndexedTransaction>,
        proposal_transactions: Vec<ProposalShortId>,
    ) -> IndexedBlock {
        IndexedBlock {
            header,
            uncles,
            commit_transactions,
            proposal_transactions,
        }
    }

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

    pub fn uncles(&self) -> &[UncleBlock] {
        &self.uncles
    }

    pub fn commit_transactions(&self) -> &[IndexedTransaction] {
        &self.commit_transactions
    }

    pub fn proposal_transactions(&self) -> &[ProposalShortId] {
        &self.proposal_transactions
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
            uncles,
            commit_transactions,
            proposal_transactions,
        } = block;
        IndexedBlock {
            header: header.into(),
            uncles,
            commit_transactions: commit_transactions
                .into_par_iter()
                .map(Into::into)
                .collect(),
            proposal_transactions,
        }
    }
}

impl From<IndexedBlock> for Block {
    fn from(block: IndexedBlock) -> Self {
        let IndexedBlock {
            header,
            uncles,
            commit_transactions,
            proposal_transactions,
        } = block;
        Block {
            header: header.header,
            uncles,
            commit_transactions: commit_transactions.into_iter().map(Into::into).collect(),
            proposal_transactions,
        }
    }
}

impl<'a> From<&'a ckb_protocol::Block> for Block {
    fn from(b: &'a ckb_protocol::Block) -> Self {
        Block {
            header: b.get_header().into(),
            uncles: b.get_uncles().iter().map(Into::into).collect(),
            commit_transactions: b.get_commit_transactions().iter().map(Into::into).collect(),
            proposal_transactions: b
                .get_proposal_transactions()
                .iter()
                .filter_map(|id| ProposalShortId::from_slice(&id))
                .collect(),
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
        let commit_transactions = b.commit_transactions.iter().map(Into::into).collect();
        block.set_commit_transactions(commit_transactions);
        let proposal_transactions = b.proposal_transactions.iter().map(|t| t.to_vec()).collect();
        block.set_proposal_transactions(proposal_transactions);
        let uncles = b.uncles.iter().map(Into::into).collect();
        block.set_uncles(uncles);
        block
    }
}

impl<'a> From<&'a IndexedBlock> for ckb_protocol::Block {
    fn from(b: &'a IndexedBlock) -> Self {
        let mut block = ckb_protocol::Block::new();
        block.set_header((&b.header).into());
        let commit_transactions = b.commit_transactions.iter().map(Into::into).collect();
        block.set_commit_transactions(commit_transactions);
        let proposal_transactions = b.proposal_transactions.iter().map(|t| t.to_vec()).collect();
        block.set_proposal_transactions(proposal_transactions);
        let uncles = b.uncles.iter().map(Into::into).collect();
        block.set_uncles(uncles);
        block
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bigint::U256;
    use header::RawHeader;
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
                txs_proposal: H256::zero(),
                difficulty: U256::zero(),
                cellbase_id: cellbase.hash(),
                uncles_hash: H256::zero(),
            },
            seal: Default::default(),
        };

        IndexedBlock {
            uncles,
            header: header.into(),
            commit_transactions: vec![cellbase],
            proposal_transactions: vec![ProposalShortId::from_slice(&[1; 10]).unwrap()],
        }
    }

    fn dummy_cellbase() -> IndexedTransaction {
        use transaction::{CellInput, CellOutput, VERSION};

        let inputs = vec![CellInput::new_cellbase_input(0)];
        let outputs = vec![CellOutput::new(0, vec![], H256::from(0))];
        Transaction::new(VERSION, vec![], inputs, outputs).into()
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
                txs_proposal: H256::zero(),
                difficulty: U256::zero(),
                cellbase_id: cellbase.hash(),
                uncles_hash: H256::zero(),
            },
            seal: Default::default(),
        };
        UncleBlock {
            header,
            cellbase: cellbase.into(),
            proposal_transactions: vec![ProposalShortId::from_slice(&[1; 10]).unwrap()],
        }
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
