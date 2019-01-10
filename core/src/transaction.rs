//! Transaction using Cell.
//! It is similar to Bitcoin Tx <https://en.bitcoin.it/wiki/Protocol_documentation#tx/>
use crate::script::Script;
use crate::BlockNumber;
pub use crate::Capacity;
use bincode::{deserialize, serialize};
use ckb_util::u64_to_bytes;
use faster_hex::hex_string;
use hash::sha3_256;
use numext_fixed_hash::H256;
use serde_derive::{Deserialize, Serialize};
use std::ops::{Deref, DerefMut};
use std::{fmt, mem};

pub const VERSION: u32 = 0;

#[derive(Clone, Serialize, Deserialize, Eq, PartialEq, Hash, Debug)]
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

    pub fn destruct(self) -> (H256, u32) {
        let OutPoint { hash, index } = self;
        (hash, index)
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

    pub fn destruct(self) -> (OutPoint, Script) {
        let CellInput {
            previous_output,
            unlock,
        } = self;
        (previous_output, unlock)
    }
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct CellOutput {
    pub capacity: Capacity,
    pub data: Vec<u8>,
    pub lock: H256,
    #[serde(rename = "type")]
    pub type_: Option<Script>,
}

impl CellOutput {
    pub fn new(capacity: Capacity, data: Vec<u8>, lock: H256, type_: Option<Script>) -> Self {
        CellOutput {
            capacity,
            data,
            lock,
            type_,
        }
    }

    pub fn data_hash(&self) -> H256 {
        sha3_256(&self.data).into()
    }

    pub fn destruct(self) -> (Capacity, Vec<u8>, H256, Option<Script>) {
        let CellOutput {
            capacity,
            data,
            lock,
            type_,
        } = self;
        (capacity, data, lock, type_)
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Debug, Default)]
pub struct Transaction {
    version: u32,
    deps: Vec<OutPoint>,
    inputs: Vec<CellInput>,
    outputs: Vec<CellOutput>,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct IndexTransaction {
    pub index: usize,
    pub transaction: Transaction,
}

impl CellOutput {
    pub fn bytes_len(&self) -> usize {
        mem::size_of::<Capacity>()
            + self.data.len()
            + H256::size_of()
            + self.type_.as_ref().map_or(0, |s| s.bytes_len())
    }
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

    pub fn into_inner(self) -> [u8; 10] {
        self.0
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

    pub fn is_cellbase(&self) -> bool {
        self.inputs.len() == 1 && self.inputs[0].previous_output.is_null()
    }

    pub fn hash(&self) -> H256 {
        sha3_256(serialize(&self).unwrap()).into()
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

    pub fn proposal_short_id(&self) -> ProposalShortId {
        ProposalShortId::from_h256(&self.hash())
    }

    pub fn get_output(&self, i: usize) -> Option<CellOutput> {
        self.outputs.get(i).cloned()
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

    pub fn build(self) -> Transaction {
        self.inner
    }
}
