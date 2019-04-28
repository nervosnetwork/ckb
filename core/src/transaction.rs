//! Transaction using Cell.
//! It is similar to Bitcoin Tx <https://en.bitcoin.it/wiki/Protocol_documentation#tx/>
use crate::script::Script;
pub use crate::Capacity;
use crate::{BlockNumber, Version};
use bincode::{deserialize, serialize};
use faster_hex::hex_string;
use hash::blake2b_256;
use numext_fixed_hash::H256;
use occupied_capacity::{HasOccupiedCapacity, OccupiedCapacity};
use serde_derive::{Deserialize, Serialize};
use std::fmt;
use std::hash::{Hash, Hasher};
use std::ops::{Deref, DerefMut};

pub const TX_VERSION: Version = 0;

#[derive(Clone, Serialize, Deserialize, Eq, PartialEq, Hash, HasOccupiedCapacity)]
pub struct OutPoint {
    // Hash of Transaction
    pub tx_hash: H256,
    // Index of output
    pub index: u32,
}

impl fmt::Debug for OutPoint {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("OutPoint")
            .field("tx_hash", &format_args!("{:#x}", self.tx_hash))
            .field("index", &self.index)
            .finish()
    }
}

impl Default for OutPoint {
    fn default() -> Self {
        OutPoint {
            tx_hash: H256::zero(),
            index: u32::max_value(),
        }
    }
}

impl OutPoint {
    pub fn new(tx_hash: H256, index: u32) -> Self {
        OutPoint { tx_hash, index }
    }

    pub fn null() -> Self {
        OutPoint::default()
    }

    pub fn is_null(&self) -> bool {
        self.tx_hash.is_zero() && self.index == u32::max_value()
    }

    pub fn destruct(self) -> (H256, u32) {
        let OutPoint { tx_hash, index } = self;
        (tx_hash, index)
    }
}

#[derive(
    Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug, HasOccupiedCapacity,
)]
pub struct CellInput {
    pub previous_output: OutPoint,
    pub since: u64,
    // Depends on whether the operation is Transform or Destroy, this is the proof to transform
    // lock or destroy lock.
    pub args: Vec<Vec<u8>>,
}

impl CellInput {
    pub fn new(previous_output: OutPoint, since: u64, args: Vec<Vec<u8>>) -> Self {
        CellInput {
            previous_output,
            since,
            args,
        }
    }

    pub fn new_cellbase_input(block_number: BlockNumber) -> Self {
        CellInput {
            previous_output: OutPoint::null(),
            since: 0,
            args: vec![block_number.to_le_bytes().to_vec()],
        }
    }

    pub fn destruct(self) -> (OutPoint, u64, Vec<Vec<u8>>) {
        let CellInput {
            previous_output,
            since,
            args,
        } = self;
        (previous_output, since, args)
    }
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, HasOccupiedCapacity)]
pub struct CellOutput {
    pub capacity: Capacity,
    #[serde(with = "serde_bytes")]
    pub data: Vec<u8>,
    pub lock: Script,
    #[serde(rename = "type")]
    pub type_: Option<Script>,
}

impl fmt::Debug for CellOutput {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("CellOutput")
            .field("capacity", &self.capacity)
            .field(
                "data",
                &format_args!("0x{}", &hex_string(&self.data).expect("hex data")),
            )
            .field("lock", &self.lock)
            .field("type", &self.type_)
            .finish()
    }
}

impl CellOutput {
    pub fn new(capacity: Capacity, data: Vec<u8>, lock: Script, type_: Option<Script>) -> Self {
        CellOutput {
            capacity,
            data,
            lock,
            type_,
        }
    }

    pub fn data_hash(&self) -> H256 {
        blake2b_256(&self.data).into()
    }

    pub fn destruct(self) -> (Capacity, Vec<u8>, Script, Option<Script>) {
        let CellOutput {
            capacity,
            data,
            lock,
            type_,
        } = self;
        (capacity, data, lock, type_)
    }

    pub fn is_occupied_capacity_overflow(&self) -> bool {
        if let Ok(cap) = self.occupied_capacity() {
            return cap > self.capacity;
        }
        true
    }
}

pub type Witness = Vec<Vec<u8>>;

#[derive(Clone, Serialize, Deserialize, Eq, Debug, Default, HasOccupiedCapacity)]
pub struct Transaction {
    version: Version,
    deps: Vec<OutPoint>,
    inputs: Vec<CellInput>,
    outputs: Vec<CellOutput>,
    //Segregated Witness to provide protection from transaction malleability.
    witnesses: Vec<Witness>,
}

#[derive(Serialize)]
struct RawTransaction<'a> {
    version: Version,
    deps: &'a [OutPoint],
    inputs: &'a [CellInput],
    outputs: &'a [CellOutput],
}

impl Hash for Transaction {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write(self.witness_hash().as_fixed_bytes())
    }
}

impl PartialEq for Transaction {
    fn eq(&self, other: &Transaction) -> bool {
        self.witness_hash() == other.witness_hash()
    }
}

impl Transaction {
    pub fn version(&self) -> u32 {
        self.version
    }

    pub fn deps(&self) -> &[OutPoint] {
        &self.deps
    }

    pub fn inputs(&self) -> &[CellInput] {
        &self.inputs
    }

    pub fn outputs(&self) -> &[CellOutput] {
        &self.outputs
    }

    pub fn witnesses(&self) -> &[Witness] {
        &self.witnesses
    }

    pub fn is_cellbase(&self) -> bool {
        self.inputs.len() == 1
            && self.inputs[0].previous_output.is_null()
            && self.inputs[0].since == 0
    }

    pub fn hash(&self) -> H256 {
        let raw = RawTransaction {
            version: self.version,
            deps: &self.deps,
            inputs: &self.inputs,
            outputs: &self.outputs,
        };
        blake2b_256(serialize(&raw).expect("Transaction serialize should not fail")).into()
    }

    pub fn witness_hash(&self) -> H256 {
        blake2b_256(serialize(&self).expect("Transaction serialize should not fail")).into()
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
            .map(|x| OutPoint::new(h.clone(), x as u32))
            .collect()
    }

    pub fn input_pts(&self) -> Vec<OutPoint> {
        self.inputs
            .iter()
            .map(|x| x.previous_output.clone())
            .collect()
    }

    pub fn dep_pts(&self) -> Vec<OutPoint> {
        self.deps.clone()
    }

    pub fn is_empty(&self) -> bool {
        self.inputs.is_empty() || self.outputs.is_empty()
    }

    // proposal_short_id
    pub fn proposal_short_id(&self) -> ProposalShortId {
        ProposalShortId::from_tx_hash(&self.hash())
    }

    pub fn get_output(&self, i: usize) -> Option<CellOutput> {
        self.outputs.get(i).cloned()
    }

    pub fn outputs_capacity(&self) -> ::occupied_capacity::Result<Capacity> {
        self.outputs
            .iter()
            .map(|output| output.capacity)
            .try_fold(Capacity::zero(), Capacity::safe_add)
    }
}

#[derive(Default)]
pub struct TransactionBuilder {
    inner: Transaction,
}

impl TransactionBuilder {
    pub fn new(bytes: &[u8]) -> Self {
        TransactionBuilder {
            inner: deserialize(bytes).expect("transaction deserializing should be ok"),
        }
    }

    pub fn transaction(mut self, transaction: Transaction) -> Self {
        self.inner = transaction;
        self
    }

    pub fn version(mut self, version: u32) -> Self {
        self.inner.version = version;
        self
    }

    pub fn dep(mut self, dep: OutPoint) -> Self {
        self.inner.deps.push(dep);
        self
    }

    pub fn deps(mut self, deps: Vec<OutPoint>) -> Self {
        self.inner.deps.extend(deps);
        self
    }

    pub fn deps_clear(mut self) -> Self {
        self.inner.deps.clear();
        self
    }

    pub fn input(mut self, input: CellInput) -> Self {
        self.inner.inputs.push(input);
        self
    }

    pub fn inputs(mut self, inputs: Vec<CellInput>) -> Self {
        self.inner.inputs.extend(inputs);
        self
    }

    pub fn inputs_clear(mut self) -> Self {
        self.inner.inputs.clear();
        self
    }

    pub fn output(mut self, output: CellOutput) -> Self {
        self.inner.outputs.push(output);
        self
    }

    pub fn outputs(mut self, outputs: Vec<CellOutput>) -> Self {
        self.inner.outputs.extend(outputs);
        self
    }

    pub fn outputs_clear(mut self) -> Self {
        self.inner.outputs.clear();
        self
    }

    pub fn witness(mut self, witness: Witness) -> Self {
        self.inner.witnesses.push(witness);
        self
    }

    pub fn witnesses(mut self, witness: Vec<Witness>) -> Self {
        self.inner.witnesses.extend(witness);
        self
    }

    pub fn witnesses_clear(mut self) -> Self {
        self.inner.witnesses.clear();
        self
    }

    pub fn build(self) -> Transaction {
        self.inner
    }
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct IndexTransaction {
    pub index: usize,
    pub transaction: Transaction,
}

#[derive(Serialize, Deserialize, Copy, Clone, PartialEq, Eq, Default, Hash)]
pub struct ProposalShortId([u8; 10]);

impl Deref for ProposalShortId {
    type Target = [u8; 10];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl fmt::Debug for ProposalShortId {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "ProposalShortId(0x{})",
            hex_string(&self.0).expect("hex proposal short id")
        )
    }
}

impl DerefMut for ProposalShortId {
    fn deref_mut(&mut self) -> &mut [u8; 10] {
        &mut self.0
    }
}

impl ProposalShortId {
    pub fn new(inner: [u8; 10]) -> Self {
        ProposalShortId(inner)
    }

    pub fn from_slice(slice: &[u8]) -> Option<Self> {
        if slice.len() == 10usize {
            let mut id = [0u8; 10];
            id.copy_from_slice(slice);
            Some(ProposalShortId(id))
        } else {
            None
        }
    }

    pub fn from_tx_hash(h: &H256) -> Self {
        let v = h.to_vec();
        let mut inner = [0u8; 10];
        inner.copy_from_slice(&v[..10]);
        ProposalShortId(inner)
    }

    pub fn hash(&self) -> H256 {
        blake2b_256(serialize(self).expect("ProposalShortId serialize should not fail")).into()
    }

    pub fn zero() -> Self {
        ProposalShortId([0; 10])
    }

    pub fn into_inner(self) -> [u8; 10] {
        self.0
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::{capacity_bytes, Capacity};

    #[test]
    fn test_tx_hash() {
        let tx = TransactionBuilder::default()
            .output(CellOutput::new(
                capacity_bytes!(5000),
                vec![1, 2, 3],
                Script::default(),
                None,
            ))
            .input(CellInput::new(OutPoint::new(H256::zero(), 0), 0, vec![]))
            .witness(vec![vec![7, 8, 9]])
            .build();

        assert_eq!(
            format!("{:x}", tx.hash()),
            "a2cfcbc6b5f4d153ea90b6e203b14f7ab1ead6eab61450f88203e414a7e68c2c"
        );
        assert_eq!(
            format!("{:x}", tx.witness_hash()),
            "4bb6ed9e544f5609749cfaa91f315adc7facecbe18b0d507330ed070fb2a4247"
        );
    }
}
