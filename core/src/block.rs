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
    transactions: Vec<Transaction>,
    proposals: Vec<ProposalShortId>,
}

impl Block {
    pub fn new(
        header: Header,
        uncles: Vec<UncleBlock>,
        transactions: Vec<Transaction>,
        proposals: Vec<ProposalShortId>,
    ) -> Block {
        Block {
            header,
            uncles,
            transactions,
            proposals,
        }
    }

    pub fn header(&self) -> &Header {
        &self.header
    }

    pub fn is_genesis(&self) -> bool {
        self.header.is_genesis()
    }

    pub fn transactions(&self) -> &[Transaction] {
        &self.transactions
    }

    pub fn proposals(&self) -> &[ProposalShortId] {
        &self.proposals
    }

    pub fn uncles(&self) -> &[UncleBlock] {
        &self.uncles
    }

    pub fn cal_uncles_hash(&self) -> H256 {
        uncles_hash(&self.uncles)
    }

    pub fn union_proposal_ids(&self) -> FnvHashSet<ProposalShortId> {
        let mut ids = FnvHashSet::default();

        ids.extend(self.proposals());

        for uc in &self.uncles {
            ids.extend(uc.proposals());
        }

        ids
    }

    pub fn cal_witnesses_root(&self) -> H256 {
        // The witness hash of cellbase transaction is assumed to be zero 0x0000....0000
        let mut witnesses = vec![H256::zero()];
        witnesses.extend(
            self.transactions()
                .iter()
                .skip(1)
                .map(Transaction::witness_hash),
        );
        merkle_root(&witnesses[..])
    }

    pub fn cal_transactions_root(&self) -> H256 {
        merkle_root(
            &self
                .transactions
                .iter()
                .map(Transaction::hash)
                .collect::<Vec<_>>(),
        )
    }

    pub fn cal_proposals_root(&self) -> H256 {
        merkle_root(
            &self
                .proposals
                .iter()
                .map(ProposalShortId::hash)
                .collect::<Vec<_>>(),
        )
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

    pub fn transaction(mut self, transaction: Transaction) -> Self {
        self.inner.transactions.push(transaction);
        self
    }

    pub fn transactions(mut self, transactions: Vec<Transaction>) -> Self {
        self.inner.transactions.extend(transactions);
        self
    }

    pub fn proposal(mut self, proposal_short_id: ProposalShortId) -> Self {
        self.inner.proposals.push(proposal_short_id);
        self
    }

    pub fn proposals(mut self, proposal_short_ids: Vec<ProposalShortId>) -> Self {
        self.inner.proposals.extend(proposal_short_ids);
        self
    }

    pub fn build(self) -> Block {
        self.inner
    }

    pub fn with_header_builder(mut self, header_builder: HeaderBuilder) -> Block {
        let transactions_root = self.inner.cal_transactions_root();
        let witnesses_root = self.inner.cal_witnesses_root();
        let proposals_root = self.inner.cal_proposals_root();
        let uncles_hash = self.inner.cal_uncles_hash();

        self.inner.header = header_builder
            .transactions_root(transactions_root)
            .proposals_root(proposals_root)
            .witnesses_root(witnesses_root)
            .uncles_hash(uncles_hash)
            .uncles_count(self.inner.uncles.len() as u32)
            .build();
        self.inner
    }
}
