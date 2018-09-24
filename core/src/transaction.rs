//! Transaction using Cell.
//! It is similar to Bitcoin Tx <https://en.bitcoin.it/wiki/Protocol_documentation#tx/>
use bigint::H256;
use bincode::serialize;
use ckb_util::u64_to_bytes;
use hash::sha3_256;
use header::BlockNumber;
use script::Script;
use std::ops::{Deref, DerefMut};

pub const VERSION: u32 = 0;

pub use Capacity;

#[derive(Clone, Copy, Serialize, Deserialize, Eq, PartialEq, Hash, Debug)]
pub struct OutPoint {
    // Hash of Transaction
    pub hash: H256,
    // Index of output
    pub index: u32,
}

impl Default for OutPoint {
    fn default() -> Self {
        OutPoint {
            hash: H256::zero(),
            index: u32::max_value(),
        }
    }
}

impl OutPoint {
    pub fn new(hash: H256, index: u32) -> Self {
        OutPoint { hash, index }
    }

    pub fn null() -> Self {
        OutPoint::default()
    }

    pub fn is_null(&self) -> bool {
        self.hash.is_zero() && self.index == u32::max_value()
    }
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct CellInput {
    pub previous_output: OutPoint,
    // Depends on whether the operation is Transform or Destroy, this is the proof to transform
    // lock or destroy lock.
    pub unlock: Script,
}

impl CellInput {
    pub fn new(previous_output: OutPoint, unlock: Script) -> Self {
        CellInput {
            previous_output,
            unlock,
        }
    }

    pub fn new_cellbase_input(block_number: BlockNumber) -> Self {
        CellInput {
            previous_output: OutPoint::null(),
            unlock: Script::new(
                0,
                Vec::new(),
                None,
                Some(u64_to_bytes(block_number.to_le()).to_vec()),
                Vec::new(),
            ),
        }
    }
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct CellOutput {
    pub capacity: Capacity,
    pub data: Vec<u8>,
    pub lock: H256,
}

impl CellOutput {
    pub fn new(capacity: Capacity, data: Vec<u8>, lock: H256) -> Self {
        CellOutput {
            capacity,
            data,
            lock,
        }
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Debug, Default)]
pub struct Transaction {
    pub version: u32,
    pub deps: Vec<OutPoint>,
    pub inputs: Vec<CellInput>,
    pub outputs: Vec<CellOutput>,
}

impl CellOutput {
    pub fn bytes_len(&self) -> usize {
        8 + self.data.len() + self.lock.len()
    }
}

#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq, Default, Hash)]
pub struct ProposalShortId([u8; 10]);

impl Deref for ProposalShortId {
    type Target = [u8; 10];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for ProposalShortId {
    fn deref_mut(&mut self) -> &mut [u8; 10] {
        &mut self.0
    }
}

impl ProposalShortId {
    pub fn from_slice(slice: &[u8]) -> Option<Self> {
        if slice.len() == 10usize {
            let mut id = [0u8; 10];
            id.copy_from_slice(slice);
            Some(ProposalShortId(id))
        } else {
            None
        }
    }

    pub fn from_h256(h: &H256) -> Self {
        let v = h.to_vec();
        let mut inner = [0u8; 10];
        inner.copy_from_slice(&v[..10]);
        ProposalShortId(inner)
    }

    pub fn hash(&self) -> H256 {
        sha3_256(serialize(self).unwrap()).into()
    }

    pub fn zero() -> Self {
        ProposalShortId([0; 10])
    }
}

impl Transaction {
    pub fn new(
        version: u32,
        deps: Vec<OutPoint>,
        inputs: Vec<CellInput>,
        outputs: Vec<CellOutput>,
    ) -> Self {
        Transaction {
            version,
            deps,
            inputs,
            outputs,
        }
    }

    pub fn is_cellbase(&self) -> bool {
        self.inputs.len() == 1 && self.inputs[0].previous_output.is_null()
    }

    pub fn hash(&self) -> H256 {
        sha3_256(serialize(self).unwrap()).into()
    }

    pub fn check_lock(&self, unlock: &[u8], lock: &[u8]) -> bool {
        // TODO: check using pubkey signature
        unlock.is_empty() || !lock.is_empty()
    }

    pub fn out_points_iter(&self) -> impl Iterator<Item = &OutPoint> {
        self.deps.iter().chain(
            self.inputs
                .iter()
                .map(|input: &CellInput| &input.previous_output),
        )
    }

    pub fn output_pts(&self) -> Vec<OutPoint> {
        let h = self.hash();
        (0..self.outputs.len())
            .map(|x| OutPoint::new(h, x as u32))
            .collect()
    }

    pub fn input_pts(&self) -> Vec<OutPoint> {
        self.inputs.iter().map(|x| x.previous_output).collect()
    }

    pub fn dep_pts(&self) -> Vec<OutPoint> {
        self.deps.clone()
    }

    pub fn is_empty(&self) -> bool {
        self.inputs.is_empty() || self.outputs.is_empty()
    }

    pub fn proposal_short_id(&self) -> ProposalShortId {
        ProposalShortId::from_h256(&self.hash())
    }

    pub fn get_output(&self, i: usize) -> Option<CellOutput> {
        self.outputs.get(i).cloned()
    }
}

impl Deref for IndexedTransaction {
    type Target = Transaction;

    fn deref(&self) -> &Self::Target {
        &self.transaction
    }
}

impl DerefMut for IndexedTransaction {
    fn deref_mut(&mut self) -> &mut Transaction {
        &mut self.transaction
    }
}

impl ::std::hash::Hash for IndexedTransaction {
    fn hash<H>(&self, state: &mut H)
    where
        H: ::std::hash::Hasher,
    {
        state.write(&self.hash);
        state.finish();
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Eq, Default)]
pub struct IndexedTransaction {
    pub transaction: Transaction,
    /// memorise hash
    hash: H256,
}

impl PartialEq for IndexedTransaction {
    fn eq(&self, other: &IndexedTransaction) -> bool {
        self.hash == other.hash
    }
}

impl IndexedTransaction {
    pub fn hash(&self) -> H256 {
        self.hash
    }

    pub fn new(transaction: Transaction, hash: H256) -> Self {
        IndexedTransaction { transaction, hash }
    }

    pub fn proposal_short_id(&self) -> ProposalShortId {
        ProposalShortId::from_h256(&self.hash())
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Eq)]
pub struct ProposalTransaction {
    pub transaction: IndexedTransaction,
    pub proposal_short_id: ProposalShortId,
}

impl ProposalTransaction {
    pub fn proposal_short_id(&self) -> ProposalShortId {
        self.proposal_short_id
    }

    pub fn new(proposal_short_id: ProposalShortId, transaction: IndexedTransaction) -> Self {
        ProposalTransaction {
            transaction,
            proposal_short_id,
        }
    }

    pub fn into_pair(self) -> (ProposalShortId, IndexedTransaction) {
        let ProposalTransaction {
            proposal_short_id,
            transaction,
        } = self;
        (proposal_short_id, transaction)
    }
}

impl PartialEq for ProposalTransaction {
    fn eq(&self, other: &ProposalTransaction) -> bool {
        self.proposal_short_id == other.proposal_short_id
    }
}

impl ::std::hash::Hash for ProposalTransaction {
    fn hash<H>(&self, state: &mut H)
    where
        H: ::std::hash::Hasher,
    {
        state.write(&self.proposal_short_id[..]);
        state.finish();
    }
}

impl From<IndexedTransaction> for ProposalTransaction {
    fn from(transaction: IndexedTransaction) -> Self {
        let proposal_short_id = transaction.proposal_short_id();
        ProposalTransaction::new(proposal_short_id, transaction)
    }
}

impl From<ProposalTransaction> for IndexedTransaction {
    fn from(proposal: ProposalTransaction) -> Self {
        let ProposalTransaction { transaction, .. } = proposal;
        transaction
    }
}

impl From<Transaction> for IndexedTransaction {
    fn from(transaction: Transaction) -> Self {
        let hash = transaction.hash();
        IndexedTransaction { transaction, hash }
    }
}

impl From<IndexedTransaction> for Transaction {
    fn from(indexed_transaction: IndexedTransaction) -> Self {
        let IndexedTransaction { transaction, .. } = indexed_transaction;
        transaction
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_transaction() -> IndexedTransaction {
        use transaction::{CellInput, CellOutput, VERSION};

        let inputs = vec![CellInput::new_cellbase_input(0)];
        let outputs = vec![CellOutput::new(0, vec![], H256::from(0))];
        Transaction::new(VERSION, vec![], inputs, outputs).into()
    }

    #[test]
    fn test_proposal_short_id() {
        let indexed_tx = dummy_transaction();
        let tx: Transaction = indexed_tx.clone().into();

        assert_eq!(tx.proposal_short_id(), indexed_tx.proposal_short_id());
    }
}
