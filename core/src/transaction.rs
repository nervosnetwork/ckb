//! Transaction using Cell.
//! It is similar to Bitcoin Tx <https://en.bitcoin.it/wiki/Protocol_documentation#tx/>
use bigint::H256;
use bincode::{deserialize, serialize};
use ckb_protocol;
use hash::{sha3_256, Sha3};
use header::BlockNumber;
use script::Script;
use std::ops::{Deref, DerefMut};

pub const VERSION: u32 = 0;

pub use Capacity;

#[derive(Clone, Copy, Serialize, Deserialize, Eq, PartialEq, Hash, Debug)]
pub struct OutPoint {
    // Hash of Transaction
    pub hash: H256,
    // Index of cell_operations
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
            unlock: Script::new(0, Vec::new(), block_number.to_le().to_bytes().to_vec()),
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

    pub fn hash(&self) -> H256 {
        sha3_256(serialize(self).unwrap()).into()
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
        let mut hash = self.hash();
        let mut sha3 = Sha3::new_sha3_256();
        let mut id = ProposalShortId::default();
        sha3.update(&hash);
        sha3.finalize(&mut hash);
        id.copy_from_slice(&hash.0[..10]);
        id
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

#[derive(Clone, Debug, Eq, Default)]
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
        let mut hash = self.hash();
        let mut sha3 = Sha3::new_sha3_256();
        let mut id = ProposalShortId::default();
        sha3.update(&hash);
        sha3.finalize(&mut hash);
        id.copy_from_slice(&hash.0[..10]);
        id
    }
}

#[derive(Clone, Debug, Eq)]
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

impl<'a> From<&'a OutPoint> for ckb_protocol::OutPoint {
    fn from(o: &'a OutPoint) -> Self {
        let mut op = ckb_protocol::OutPoint::new();
        op.set_hash(o.hash.to_vec());
        op.set_index(o.index);
        op
    }
}

impl<'a> From<&'a ckb_protocol::OutPoint> for OutPoint {
    fn from(o: &'a ckb_protocol::OutPoint) -> Self {
        Self {
            hash: H256::from(o.get_hash()),
            index: o.get_index(),
        }
    }
}

impl<'a> From<&'a ckb_protocol::CellInput> for CellInput {
    fn from(c: &'a ckb_protocol::CellInput) -> Self {
        Self {
            previous_output: c.get_previous_output().into(),
            unlock: deserialize(c.get_unlock()).unwrap(),
        }
    }
}

impl<'a> From<&'a CellInput> for ckb_protocol::CellInput {
    fn from(c: &'a CellInput) -> Self {
        let mut ci = ckb_protocol::CellInput::new();
        ci.set_previous_output((&c.previous_output).into());
        ci.set_unlock(serialize(&c.unlock).unwrap());
        ci
    }
}

impl From<CellInput> for ckb_protocol::CellInput {
    fn from(c: CellInput) -> Self {
        let CellInput {
            previous_output,
            unlock,
        } = c;
        let mut ci = ckb_protocol::CellInput::new();
        ci.set_previous_output((&previous_output).into());
        ci.set_unlock(serialize(&unlock).unwrap());
        ci
    }
}

/// stupid proto3
impl<'a> From<&'a ckb_protocol::CellOutput> for CellOutput {
    fn from(c: &'a ckb_protocol::CellOutput) -> Self {
        Self {
            capacity: c.get_capacity(),
            data: c.get_data().to_vec(),
            lock: c.get_lock().into(),
        }
    }
}

impl<'a> From<&'a CellOutput> for ckb_protocol::CellOutput {
    fn from(c: &'a CellOutput) -> Self {
        let mut co = ckb_protocol::CellOutput::new();
        co.set_capacity(c.capacity);
        co.set_data(c.data.clone());
        co.set_lock(c.lock.to_vec());
        co
    }
}

impl From<CellOutput> for ckb_protocol::CellOutput {
    fn from(c: CellOutput) -> Self {
        let CellOutput {
            capacity,
            data,
            lock,
        } = c;
        let mut co = ckb_protocol::CellOutput::new();
        co.set_capacity(capacity);
        co.set_data(data);
        co.set_lock(lock.to_vec());
        co
    }
}

impl<'a> From<&'a ckb_protocol::Transaction> for Transaction {
    fn from(t: &'a ckb_protocol::Transaction) -> Self {
        Self {
            version: t.get_version(),
            deps: t.get_deps().iter().map(Into::into).collect(),
            inputs: t.get_inputs().iter().map(Into::into).collect(),
            outputs: t.get_outputs().iter().map(Into::into).collect(),
        }
    }
}

impl<'a> From<&'a ckb_protocol::Transaction> for IndexedTransaction {
    fn from(t: &'a ckb_protocol::Transaction) -> Self {
        let tx: Transaction = t.into();
        tx.into()
    }
}

impl<'a> From<&'a ckb_protocol::Transaction> for ProposalTransaction {
    fn from(t: &'a ckb_protocol::Transaction) -> Self {
        let idx_tx: IndexedTransaction = t.into();
        idx_tx.into()
    }
}

impl<'a> From<&'a Transaction> for ckb_protocol::Transaction {
    fn from(t: &'a Transaction) -> Self {
        let mut tx = ckb_protocol::Transaction::new();
        tx.set_version(t.version);
        tx.set_inputs(t.inputs.iter().map(Into::into).collect());
        tx.set_outputs(t.outputs.iter().map(Into::into).collect());
        tx
    }
}

impl<'a> From<&'a IndexedTransaction> for ckb_protocol::Transaction {
    fn from(t: &'a IndexedTransaction) -> Self {
        let tx = &t.transaction;
        tx.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use protobuf;
    use protobuf::Message;

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

    #[test]
    fn test_proto() {
        let tx = dummy_transaction();
        let proto_tx: ckb_protocol::Transaction = (&tx).into();
        let message = proto_tx.write_to_bytes().unwrap();
        let decoded_proto_tx =
            protobuf::parse_from_bytes::<ckb_protocol::Transaction>(&message).unwrap();
        assert_eq!(proto_tx, decoded_proto_tx);
        let decoded_tx: IndexedTransaction = (&decoded_proto_tx).into();
        assert_eq!(tx, decoded_tx);
    }
}
