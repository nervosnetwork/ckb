use super::header::{Header, IndexedHeader};
use super::transaction::{IndexedTransaction, ProposalShortId, Transaction};
use bigint::H256;
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

    pub fn union_proposal_ids(&self) -> Vec<ProposalShortId> {
        let mut ids = FnvHashSet::default();

        ids.extend(self.proposal_transactions.clone());

        for uc in &self.uncles {
            ids.extend(uc.proposal_transactions.clone());
        }

        ids.into_iter().collect()
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
