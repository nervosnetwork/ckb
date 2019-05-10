//! Transaction using Cell.
//! It is similar to Bitcoin Tx <https://en.bitcoin.it/wiki/Protocol_documentation#tx/>
use crate::script::Script;
pub use crate::Capacity;
use crate::{BlockNumber, Bytes, Version};
use bincode::{deserialize, serialize};
use faster_hex::hex_string;
use hash::blake2b_256;
use numext_fixed_hash::H256;
use occupied_capacity::{HasOccupiedCapacity, OccupiedCapacity};
use serde_derive::{Deserialize, Serialize};
use std::fmt;
use std::hash::{Hash, Hasher};
use std::mem;
use std::ops::{Deref, DerefMut};

pub const TX_VERSION: Version = 0;

#[derive(Clone, Serialize, Deserialize, Eq, PartialEq, Hash, HasOccupiedCapacity)]
pub struct CellOutPoint {
    // Hash of Transaction
    pub tx_hash: H256,
    // Index of output
    pub index: u32,
}

impl fmt::Debug for CellOutPoint {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("CellOutPoint")
            .field("tx_hash", &format_args!("{:#x}", self.tx_hash))
            .field("index", &self.index)
            .finish()
    }
}

impl Default for CellOutPoint {
    fn default() -> Self {
        CellOutPoint {
            tx_hash: H256::zero(),
            index: u32::max_value(),
        }
    }
}

impl CellOutPoint {
    pub fn destruct(self) -> (H256, u32) {
        let CellOutPoint { tx_hash, index } = self;
        (tx_hash, index)
    }
}

#[derive(Clone, Default, Serialize, Deserialize, Eq, PartialEq, Hash, HasOccupiedCapacity)]
pub struct OutPoint {
    pub cell: Option<CellOutPoint>,
    pub block_hash: Option<H256>,
}

impl fmt::Debug for OutPoint {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("OutPoint")
            .field("cell", &self.cell)
            .field("block_hash", &self.block_hash)
            .finish()
    }
}

impl OutPoint {
    pub fn new(block_hash: H256, tx_hash: H256, index: u32) -> Self {
        OutPoint {
            block_hash: Some(block_hash),
            cell: Some(CellOutPoint { tx_hash, index }),
        }
    }

    pub fn new_cell(tx_hash: H256, index: u32) -> Self {
        OutPoint {
            block_hash: None,
            cell: Some(CellOutPoint { tx_hash, index }),
        }
    }

    pub fn new_block_hash(block_hash: H256) -> Self {
        OutPoint {
            block_hash: Some(block_hash),
            cell: None,
        }
    }

    pub fn null() -> Self {
        OutPoint::default()
    }

    pub fn is_null(&self) -> bool {
        self.cell.is_none() && self.block_hash.is_none()
    }

    pub const fn serialized_size() -> usize {
        H256::size_of() + mem::size_of::<u32>()
    }

    pub fn destruct(self) -> (Option<H256>, Option<CellOutPoint>) {
        let OutPoint { block_hash, cell } = self;
        (block_hash, cell)
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
    pub args: Vec<Bytes>,
}

impl CellInput {
    pub fn new(previous_output: OutPoint, since: u64, args: Vec<Bytes>) -> Self {
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
            args: vec![Bytes::from(block_number.to_le_bytes().to_vec())],
        }
    }

    pub fn destruct(self) -> (OutPoint, u64, Vec<Bytes>) {
        let CellInput {
            previous_output,
            since,
            args,
        } = self;
        (previous_output, since, args)
    }

    pub fn serialized_size(&self) -> usize {
        OutPoint::serialized_size()
            + mem::size_of::<u64>()
            + self.args.iter().map(Bytes::len).sum::<usize>()
    }
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, HasOccupiedCapacity)]
pub struct CellOutput {
    pub capacity: Capacity,
    pub data: Bytes,
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
    pub fn new(capacity: Capacity, data: Bytes, lock: Script, type_: Option<Script>) -> Self {
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

    pub fn serialized_size(&self) -> usize {
        mem::size_of::<u64>()
            + self.data.len()
            + self.lock.serialized_size()
            + self
                .type_
                .as_ref()
                .map(Script::serialized_size)
                .unwrap_or(0)
    }

    pub fn destruct(self) -> (Capacity, Bytes, Script, Option<Script>) {
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

pub type Witness = Vec<Bytes>;

#[derive(Clone, Serialize, Eq, Debug, HasOccupiedCapacity)]
pub struct Transaction {
    version: Version,
    deps: Vec<OutPoint>,
    inputs: Vec<CellInput>,
    outputs: Vec<CellOutput>,
    //Segregated Witness to provide protection from transaction malleability.
    witnesses: Vec<Witness>,
    #[serde(skip)]
    #[free_capacity]
    hash: H256,
    #[serde(skip)]
    #[free_capacity]
    witness_hash: H256,
}

// The order of fields should be same as TransactionStoredOwned
#[derive(Serialize)]
pub struct TransactionStored<'a> {
    version: Version,
    deps: &'a [OutPoint],
    inputs: &'a [CellInput],
    outputs: &'a [CellOutput],
    witnesses: &'a [Witness],
    hash: &'a H256,
    witness_hash: &'a H256,
}

// The order of fields should be same as TransactionStored
#[derive(Deserialize)]
struct TransactionStoredOwned {
    version: Version,
    deps: Vec<OutPoint>,
    inputs: Vec<CellInput>,
    outputs: Vec<CellOutput>,
    witnesses: Vec<Witness>,
    hash: H256,
    witness_hash: H256,
}

impl From<TransactionStoredOwned> for Transaction {
    #[inline]
    fn from(tx: TransactionStoredOwned) -> Self {
        let TransactionStoredOwned {
            version,
            deps,
            inputs,
            outputs,
            witnesses,
            hash,
            witness_hash,
        } = tx;
        Self {
            version,
            deps,
            inputs,
            outputs,
            witnesses,
            hash,
            witness_hash,
        }
    }
}

impl<'de> serde::de::Deserialize<'de> for Transaction {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::de::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(field_identifier, rename_all = "snake_case")]
        enum Field {
            Version,
            Deps,
            Inputs,
            Outputs,
            Witnesses,
        }

        struct InnerVisitor;

        impl<'de> serde::de::Visitor<'de> for InnerVisitor {
            type Value = Transaction;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("struct Transaction")
            }

            fn visit_seq<V>(self, mut seq: V) -> Result<Self::Value, V::Error>
            where
                V: serde::de::SeqAccess<'de>,
            {
                let version = seq
                    .next_element()?
                    .ok_or_else(|| serde::de::Error::invalid_length(0, &self))?;
                let deps = seq
                    .next_element()?
                    .ok_or_else(|| serde::de::Error::invalid_length(1, &self))?;
                let inputs = seq
                    .next_element()?
                    .ok_or_else(|| serde::de::Error::invalid_length(2, &self))?;
                let outputs = seq
                    .next_element()?
                    .ok_or_else(|| serde::de::Error::invalid_length(3, &self))?;
                let witnesses = seq
                    .next_element()?
                    .ok_or_else(|| serde::de::Error::invalid_length(4, &self))?;
                Ok(Self::Value::new(version, deps, inputs, outputs, witnesses))
            }

            fn visit_map<V>(self, mut map: V) -> Result<Self::Value, V::Error>
            where
                V: serde::de::MapAccess<'de>,
            {
                let mut version = None;
                let mut deps = None;
                let mut inputs = None;
                let mut outputs = None;
                let mut witnesses = None;
                while let Some(key) = map.next_key()? {
                    match key {
                        Field::Version => {
                            if version.is_some() {
                                return Err(serde::de::Error::duplicate_field("version"));
                            }
                            version = Some(map.next_value()?);
                        }
                        Field::Deps => {
                            if deps.is_some() {
                                return Err(serde::de::Error::duplicate_field("deps"));
                            }
                            deps = Some(map.next_value()?);
                        }
                        Field::Inputs => {
                            if inputs.is_some() {
                                return Err(serde::de::Error::duplicate_field("inputs"));
                            }
                            inputs = Some(map.next_value()?);
                        }
                        Field::Outputs => {
                            if outputs.is_some() {
                                return Err(serde::de::Error::duplicate_field("outputs"));
                            }
                            outputs = Some(map.next_value()?);
                        }
                        Field::Witnesses => {
                            if witnesses.is_some() {
                                return Err(serde::de::Error::duplicate_field("witnesses"));
                            }
                            witnesses = Some(map.next_value()?);
                        }
                    }
                }
                let version = version.ok_or_else(|| serde::de::Error::missing_field("version"))?;
                let deps = deps.ok_or_else(|| serde::de::Error::missing_field("deps"))?;
                let inputs = inputs.ok_or_else(|| serde::de::Error::missing_field("inputs"))?;
                let outputs = outputs.ok_or_else(|| serde::de::Error::missing_field("outputs"))?;
                let witnesses =
                    witnesses.ok_or_else(|| serde::de::Error::missing_field("witnesses"))?;
                Ok(Self::Value::new(version, deps, inputs, outputs, witnesses))
            }
        }

        const FIELDS: &[&str] = &["version", "deps", "inputs", "outputs", "witnesses"];
        deserializer.deserialize_struct("Transaction", FIELDS, InnerVisitor)
    }
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
    /// # Warning
    ///
    /// When using this method, the caller should ensure the input hashes is right, or the caller
    /// will get a incorrect Transaction.
    pub unsafe fn from_bytes_unchecked(bytes: &[u8]) -> Self {
        let tx_stored: TransactionStoredOwned =
            deserialize(bytes).expect("stored transaction deserializing should be ok");
        tx_stored.into()
    }

    pub(crate) fn new(
        version: Version,
        deps: Vec<OutPoint>,
        inputs: Vec<CellInput>,
        outputs: Vec<CellOutput>,
        witnesses: Vec<Witness>,
    ) -> Self {
        let raw = RawTransaction {
            version,
            deps: &deps,
            inputs: &inputs,
            outputs: &outputs,
        };
        let hash =
            blake2b_256(serialize(&raw).expect("RawTransaction serialize should not fail")).into();
        let mut tx = Self {
            version,
            deps,
            inputs,
            outputs,
            witnesses,
            hash,
            witness_hash: H256::zero(),
        };
        let witness_hash =
            blake2b_256(serialize(&tx).expect("Transaction serialize should not fail")).into();
        tx.witness_hash = witness_hash;
        tx
    }

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

    pub fn hash(&self) -> &H256 {
        &self.hash
    }

    pub fn witness_hash(&self) -> &H256 {
        &self.witness_hash
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
            .map(|x| OutPoint::new_cell(h.clone(), x as u32))
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

    pub fn serialized_size(&self) -> usize {
        mem::size_of::<Version>()
            + self.deps.len() * OutPoint::serialized_size()
            + self
                .inputs
                .iter()
                .map(CellInput::serialized_size)
                .sum::<usize>()
            + self
                .outputs
                .iter()
                .map(CellOutput::serialized_size)
                .sum::<usize>()
            + self
                .witnesses
                .iter()
                .flat_map(|witness| witness.iter().map(Vec::len))
                .sum::<usize>()
    }

    pub fn to_stored(&self) -> TransactionStored {
        TransactionStored {
            version: self.version,
            deps: &self.deps[..],
            inputs: &self.inputs[..],
            outputs: &self.outputs[..],
            witnesses: &self.witnesses[..],
            hash: &self.hash,
            witness_hash: &self.witness_hash,
        }
    }
}

#[derive(Default)]
pub struct TransactionBuilder {
    version: Version,
    deps: Vec<OutPoint>,
    inputs: Vec<CellInput>,
    outputs: Vec<CellOutput>,
    witnesses: Vec<Witness>,
}

impl TransactionBuilder {
    pub fn new(bytes: &[u8]) -> Self {
        let Transaction {
            version,
            deps,
            inputs,
            outputs,
            witnesses,
            ..
        } = deserialize(bytes).expect("transaction deserializing should be ok");
        Self {
            version,
            deps,
            inputs,
            outputs,
            witnesses,
        }
    }

    pub fn from_transaction(transaction: Transaction) -> Self {
        let Transaction {
            version,
            deps,
            inputs,
            outputs,
            witnesses,
            ..
        } = transaction;
        Self {
            version,
            deps,
            inputs,
            outputs,
            witnesses,
        }
    }

    pub fn version(mut self, version: u32) -> Self {
        self.version = version;
        self
    }

    pub fn dep(mut self, dep: OutPoint) -> Self {
        self.deps.push(dep);
        self
    }

    pub fn deps(mut self, deps: Vec<OutPoint>) -> Self {
        self.deps.extend(deps);
        self
    }

    pub fn deps_clear(mut self) -> Self {
        self.deps.clear();
        self
    }

    pub fn input(mut self, input: CellInput) -> Self {
        self.inputs.push(input);
        self
    }

    pub fn inputs(mut self, inputs: Vec<CellInput>) -> Self {
        self.inputs.extend(inputs);
        self
    }

    pub fn inputs_clear(mut self) -> Self {
        self.inputs.clear();
        self
    }

    pub fn output(mut self, output: CellOutput) -> Self {
        self.outputs.push(output);
        self
    }

    pub fn outputs(mut self, outputs: Vec<CellOutput>) -> Self {
        self.outputs.extend(outputs);
        self
    }

    pub fn outputs_clear(mut self) -> Self {
        self.outputs.clear();
        self
    }

    pub fn witness(mut self, witness: Witness) -> Self {
        self.witnesses.push(witness);
        self
    }

    pub fn witnesses(mut self, witness: Vec<Witness>) -> Self {
        self.witnesses.extend(witness);
        self
    }

    pub fn witnesses_clear(mut self) -> Self {
        self.witnesses.clear();
        self
    }

    pub fn build(self) -> Transaction {
        let Self {
            version,
            deps,
            inputs,
            outputs,
            witnesses,
        } = self;
        Transaction::new(version, deps, inputs, outputs, witnesses)
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

    pub fn zero() -> Self {
        ProposalShortId([0; 10])
    }

    pub fn into_inner(self) -> [u8; 10] {
        self.0
    }

    pub const fn serialized_size() -> usize {
        mem::size_of::<[u8; 10]>()
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::{capacity_bytes, Bytes, Capacity};

    #[test]
    fn test_tx_hash() {
        let tx = TransactionBuilder::default()
            .output(CellOutput::new(
                capacity_bytes!(5000),
                Bytes::from(vec![1, 2, 3]),
                Script::default(),
                None,
            ))
            .input(CellInput::new(
                OutPoint::new_cell(H256::zero(), 0),
                0,
                vec![],
            ))
            .witness(vec![Bytes::from(vec![7, 8, 9])])
            .build();

        assert_eq!(
            format!("{:x}", tx.hash()),
            "d5af472fc9cae95c8c3fe440ad72b83ea3e1b1f150aaf5dd19742c0acebace89"
        );
        assert_eq!(
            format!("{:x}", tx.witness_hash()),
            "01da42e3575e48f2f40b63b598bd97ffcb3d089049308a676cad2cb791422f2c"
        );
    }
}
