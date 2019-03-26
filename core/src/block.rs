use crate::header::{Header, HeaderBuilder};
use crate::transaction::{ProposalShortId, Transaction};
use crate::uncle::{uncles_hash, UncleBlock};
use ckb_merkle_tree::merkle_root;
use fnv::FnvHashSet;
use numext_fixed_hash::H256;
use serde_derive::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize, Eq, Default, Debug)]
pub struct Block {
    header: Header,
    uncles: Vec<UncleBlock>,
    commit_transactions: Vec<Transaction>,
    proposal_transactions: Vec<ProposalShortId>,
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

    pub fn union_proposal_ids(&self) -> Vec<ProposalShortId> {
        let mut ids = FnvHashSet::default();

        ids.extend(self.proposal_transactions());

        for uc in &self.uncles {
            ids.extend(uc.proposal_transactions());
        }

        ids.into_iter().collect()
    }
}

impl ::std::hash::Hash for Block {
    fn hash<H>(&self, state: &mut H)
    where
        H: ::std::hash::Hasher,
    {
        state.write(&self.header.hash().as_bytes());
        state.finish();
    }
}

impl PartialEq for Block {
    fn eq(&self, other: &Block) -> bool {
        self.header().hash() == other.header().hash()
    }
}

#[derive(Default)]
pub struct BlockBuilder {
    inner: Block,
}

impl BlockBuilder {
    pub fn block(mut self, block: Block) -> Self {
        self.inner = block;
        self
    }

    pub fn header(mut self, header: Header) -> Self {
        self.inner.header = header;
        self
    }

    pub fn uncle(mut self, uncle: UncleBlock) -> Self {
        self.inner.uncles.push(uncle);
        self
    }

    pub fn uncles(mut self, uncles: Vec<UncleBlock>) -> Self {
        self.inner.uncles.extend(uncles);
        self
    }

    pub fn commit_transaction(mut self, transaction: Transaction) -> Self {
        self.inner.commit_transactions.push(transaction);
        self
    }

    pub fn commit_transactions(mut self, transactions: Vec<Transaction>) -> Self {
        self.inner.commit_transactions.extend(transactions);
        self
    }

    pub fn proposal_transaction(mut self, proposal_short_id: ProposalShortId) -> Self {
        self.inner.proposal_transactions.push(proposal_short_id);
        self
    }

    pub fn proposal_transactions(mut self, proposal_short_ids: Vec<ProposalShortId>) -> Self {
        self.inner.proposal_transactions.extend(proposal_short_ids);
        self
    }

    pub fn build(self) -> Block {
        self.inner
    }

    pub fn with_header_builder(mut self, header_builder: HeaderBuilder) -> Block {
        let txs_commit = merkle_root(
            &self
                .inner
                .commit_transactions
                .iter()
                .map(|t| t.hash())
                .collect::<Vec<_>>(),
        );

        // The witness hash of cellbase transaction is assumed to be zero 0x0000....0000
        let mut witnesses = vec![H256::zero()];
        witnesses.extend(
            self.inner
                .commit_transactions()
                .iter()
                .skip(1)
                .map(|tx| tx.witness_hash()),
        );
        let witnesses_root = merkle_root(&witnesses[..]);

        let txs_proposal = merkle_root(
            &self
                .inner
                .proposal_transactions
                .iter()
                .map(|t| t.hash())
                .collect::<Vec<_>>(),
        );

        let uncles_hash = uncles_hash(&self.inner.uncles);

        self.inner.header = header_builder
            .txs_commit(txs_commit)
            .txs_proposal(txs_proposal)
            .witnesses_root(witnesses_root)
            .uncles_hash(uncles_hash)
            .uncles_count(self.inner.uncles.len() as u32)
            .build();
        self.inner
    }
}
