//! Transaction using Cell.
//! It is similar to Bitcoin Tx <https://en.bitcoin.it/wiki/Protocol_documentation#tx/>
use crate::script::Script;
use crate::{BlockNumber, Bytes, Version};
use bincode::serialize;
use ckb_hash::blake2b_256;
use ckb_occupied_capacity::{Capacity, Result as CapacityResult};
use faster_hex::hex_string;
use numext_fixed_hash::H256;
use serde_derive::{Deserialize, Serialize};
use std::convert::TryInto;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::mem;
use std::ops::{Deref, DerefMut};

pub const TX_VERSION: Version = 0;

pub struct CellKey([u8; 36]);

impl CellKey {
    pub fn calculate(tx_hash: &H256, index: u32) -> Self {
        let mut key: [u8; 36] = [0; 36];
        key[..32].copy_from_slice(tx_hash.as_bytes());
        key[32..36].copy_from_slice(&index.to_le_bytes());
        CellKey(key)
    }

    pub fn recover(&self) -> OutPoint {
        Self::deconstruct(&self.0)
    }

    pub fn deconstruct(bytes: &[u8]) -> OutPoint {
        let tx_hash = H256::from_slice(&bytes[..32]).expect("should not be failed");
        let le_bytes: [u8; 4] = bytes[32..36].try_into().expect("should not be failed");
        let index = u32::from_le_bytes(le_bytes);
        OutPoint { tx_hash, index }
    }
}

impl AsRef<[u8]> for CellKey {
    fn as_ref(&self) -> &[u8] {
        &self.0[..]
    }
}

#[derive(Clone, Serialize, Deserialize, Eq, PartialEq, Hash)]
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
            index: 0,
        }
    }
}

impl OutPoint {
    pub fn new(tx_hash: H256, index: u32) -> OutPoint {
        OutPoint { tx_hash, index }
    }

    pub fn null() -> OutPoint {
        OutPoint {
            tx_hash: H256::zero(),
            index: u32::max_value(),
        }
    }

    pub fn is_null(&self) -> bool {
        self.tx_hash.is_zero() && self.index == u32::max_value()
    }

    pub fn destruct(self) -> (H256, u32) {
        let OutPoint { tx_hash, index } = self;
        (tx_hash, index)
    }

    pub const fn serialized_size() -> usize {
        H256::size_of() + mem::size_of::<u32>()
    }

    pub fn cell_key(&self) -> CellKey {
        CellKey::calculate(&self.tx_hash, self.index)
    }
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct CellDep {
    out_point: OutPoint,
    is_dep_group: bool,
}

impl CellDep {
    pub fn new(out_point: OutPoint, is_dep_group: bool) -> CellDep {
        CellDep {
            out_point,
            is_dep_group,
        }
    }

    pub fn new_cell(out_point: OutPoint) -> CellDep {
        Self::new(out_point, false)
    }

    pub fn new_group(out_point: OutPoint) -> CellDep {
        Self::new(out_point, true)
    }

    pub fn out_point(&self) -> &OutPoint {
        &self.out_point
    }

    pub fn is_dep_group(&self) -> bool {
        self.is_dep_group
    }

    pub fn into_inner(self) -> OutPoint {
        self.out_point
    }

    pub const fn serialized_size() -> usize {
        OutPoint::serialized_size() + mem::size_of::<bool>()
    }
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct CellInput {
    pub previous_output: OutPoint,
    pub since: u64,
}

impl CellInput {
    pub fn new(previous_output: OutPoint, since: u64) -> Self {
        CellInput {
            previous_output,
            since,
        }
    }

    pub fn new_cellbase_input(block_number: BlockNumber) -> Self {
        CellInput {
            previous_output: OutPoint::null(),
            since: block_number,
        }
    }

    pub fn destruct(self) -> (OutPoint, u64) {
        let CellInput {
            previous_output,
            since,
        } = self;
        (previous_output, since)
    }

    pub fn serialized_size() -> usize {
        OutPoint::serialized_size() + mem::size_of::<u64>()
    }
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct CellOutput {
    pub capacity: Capacity,
    pub data_hash: H256,
    pub lock: Script,
    #[serde(rename = "type")]
    pub type_: Option<Script>,
}

impl fmt::Debug for CellOutput {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("CellOutput")
            .field("capacity", &self.capacity)
            .field("data_hash", &format_args!("{:#x}", &self.data_hash))
            .field("lock", &self.lock)
            .field("type", &self.type_)
            .finish()
    }
}

impl CellOutput {
    pub fn new(capacity: Capacity, data_hash: H256, lock: Script, type_: Option<Script>) -> Self {
        CellOutput {
            capacity,
            data_hash,
            lock,
            type_,
        }
    }

    pub fn calculate_data_hash(data: &Bytes) -> H256 {
        if data.is_empty() {
            H256::zero()
        } else {
            blake2b_256(data).into()
        }
    }

    pub fn data_hash(&self) -> &H256 {
        &self.data_hash
    }

    pub fn serialized_size(&self) -> usize {
        mem::size_of::<u64>()
            + 32
            + self.lock.serialized_size()
            + self
                .type_
                .as_ref()
                .map(Script::serialized_size)
                .unwrap_or(0)
    }

    pub fn destruct(self) -> (Capacity, H256, Script, Option<Script>) {
        let CellOutput {
            capacity,
            data_hash,
            lock,
            type_,
            ..
        } = self;
        (capacity, data_hash, lock, type_)
    }

    pub fn occupied_capacity(&self, data_capacity: Capacity) -> CapacityResult<Capacity> {
        Capacity::bytes(8)
            .and_then(|x| x.safe_add(data_capacity))
            .and_then(|x| self.lock.occupied_capacity().and_then(|y| y.safe_add(x)))
            .and_then(|x| {
                self.type_
                    .as_ref()
                    .map(Script::occupied_capacity)
                    .transpose()
                    .and_then(|y| y.unwrap_or_else(Capacity::zero).safe_add(x))
            })
    }

    pub fn is_lack_of_capacity(&self, data_capacity: Capacity) -> CapacityResult<bool> {
        self.occupied_capacity(data_capacity)
            .map(|cap| cap > self.capacity)
    }
}

pub struct CellOutputBuilder {
    pub capacity: Capacity,
    pub data_hash: H256,
    pub lock: Script,
    pub type_: Option<Script>,
}

impl Default for CellOutputBuilder {
    fn default() -> Self {
        Self {
            capacity: Default::default(),
            data_hash: Default::default(),
            lock: Default::default(),
            type_: None,
        }
    }
}

impl CellOutputBuilder {
    pub fn from_data(data: &Bytes) -> Self {
        Self::default().data_hash(CellOutput::calculate_data_hash(data))
    }

    pub fn capacity(mut self, capacity: Capacity) -> Self {
        self.capacity = capacity;
        self
    }

    pub fn data_hash(mut self, data_hash: H256) -> Self {
        self.data_hash = data_hash;
        self
    }

    pub fn lock(mut self, lock: Script) -> Self {
        self.lock = lock;
        self
    }

    pub fn type_(mut self, type_: Option<Script>) -> Self {
        self.type_ = type_;
        self
    }

    pub fn build(self) -> CellOutput {
        let Self {
            capacity,
            data_hash,
            lock,
            type_,
        } = self;
        CellOutput::new(capacity, data_hash, lock, type_)
    }
}

pub type Witness = Vec<Bytes>;

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Debug, Default)]
pub struct Deps {
    cells: Vec<CellDep>,
    headers: Vec<H256>,
}

impl Deps {
    pub fn new(cells: Vec<CellDep>, headers: Vec<H256>) -> Deps {
        Deps { cells, headers }
    }

    pub fn cells(&self) -> &[CellDep] {
        &self.cells
    }

    pub fn headers(&self) -> &[H256] {
        &self.headers
    }

    pub fn destruct(self) -> (Vec<CellDep>, Vec<H256>) {
        (self.cells, self.headers)
    }

    pub fn serialized_size(&self) -> usize {
        CellDep::serialized_size() * self.cells.len() + 4 + H256::size_of() * self.headers.len() + 4
    }
}

#[derive(Clone, Serialize, Eq, Debug)]
pub struct Transaction {
    version: Version,
    deps: Deps,
    inputs: Vec<CellInput>,
    outputs: Vec<CellOutput>,
    #[serde(skip)]
    outputs_data: Vec<Bytes>,
    //Segregated Witness to provide protection from transaction malleability.
    witnesses: Vec<Witness>,
    #[serde(skip)]
    hash: H256,
    #[serde(skip)]
    witness_hash: H256,
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
                    .ok_or_else(|| serde::de::Error::invalid_length(5, &self))?;
                Ok(Self::Value::new(
                    version,
                    deps,
                    inputs,
                    outputs,
                    Default::default(),
                    witnesses,
                ))
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
                Ok(Self::Value::new(
                    version,
                    deps,
                    inputs,
                    outputs,
                    Default::default(),
                    witnesses,
                ))
            }
        }

        const FIELDS: &[&str] = &["version", "deps", "inputs", "outputs", "witnesses"];
        deserializer.deserialize_struct("Transaction", FIELDS, InnerVisitor)
    }
}

#[derive(Serialize)]
struct RawTransaction<'a> {
    version: Version,
    deps: &'a Deps,
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
    pub(crate) fn new(
        version: Version,
        deps: Deps,
        inputs: Vec<CellInput>,
        outputs: Vec<CellOutput>,
        outputs_data: Vec<Bytes>,
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
            outputs_data,
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

    pub fn deps(&self) -> &Deps {
        &self.deps
    }

    pub fn inputs(&self) -> &[CellInput] {
        &self.inputs
    }

    pub fn outputs(&self) -> &[CellOutput] {
        &self.outputs
    }

    pub fn outputs_data(&self) -> &[Bytes] {
        &self.outputs_data
    }

    pub fn witnesses(&self) -> &[Witness] {
        &self.witnesses
    }

    // one-in one-out one-wit
    pub fn is_cellbase(&self) -> bool {
        self.inputs.len() == 1
            && self.outputs.len() == 1
            && self.witnesses.len() == 1
            && self.inputs[0].previous_output.is_null()
    }

    pub fn hash(&self) -> &H256 {
        &self.hash
    }

    pub fn witness_hash(&self) -> &H256 {
        &self.witness_hash
    }

    pub fn output_pts(&self) -> Vec<OutPoint> {
        let h = self.hash();
        (0..self.outputs.len())
            .map(|x| OutPoint::new(h.clone(), x as u32))
            .collect()
    }

    pub fn input_pts_iter(&self) -> impl Iterator<Item = &OutPoint> {
        self.inputs.iter().map(|x| &x.previous_output)
    }

    pub fn cell_deps_iter(&self) -> impl Iterator<Item = &CellDep> {
        self.deps.cells().iter()
    }

    pub fn header_deps_iter(&self) -> impl Iterator<Item = &H256> {
        self.deps.headers().iter()
    }

    pub fn outputs_with_data_iter(&self) -> impl Iterator<Item = (&CellOutput, &Bytes)> {
        self.outputs.iter().zip(&self.outputs_data)
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

    pub fn get_output_with_data(&self, i: usize) -> Option<(CellOutput, Bytes)> {
        self.outputs.get(i).cloned().map(|output| {
            let output_data = self.outputs_data.get(i).cloned().expect("must exists");
            (output, output_data)
        })
    }

    pub fn outputs_capacity(&self) -> CapacityResult<Capacity> {
        self.outputs
            .iter()
            .map(|output| output.capacity)
            .try_fold(Capacity::zero(), Capacity::safe_add)
    }

    pub fn serialized_size(&self) -> usize {
        mem::size_of::<Version>()
            + self.deps.serialized_size()
            + CellInput::serialized_size() * self.inputs.len()
            + 4
            + self
                .outputs
                .iter()
                .map(CellOutput::serialized_size)
                .sum::<usize>()
            + 4
            + self.outputs_data.iter().map(Bytes::len).sum::<usize>()
            + 4
            + self
                .witnesses
                .iter()
                .flat_map(|witness| witness.iter().map(Bytes::len))
                .sum::<usize>()
            + 4
    }
}

pub struct TransactionBuilder {
    version: Version,
    deps: Deps,
    inputs: Vec<CellInput>,
    outputs: Vec<CellOutput>,
    outputs_data: Vec<Bytes>,
    witnesses: Vec<Witness>,
}

impl Default for TransactionBuilder {
    fn default() -> Self {
        Self {
            version: TX_VERSION,
            deps: Default::default(),
            inputs: Default::default(),
            outputs: Default::default(),
            outputs_data: Default::default(),
            witnesses: Default::default(),
        }
    }
}

impl TransactionBuilder {
    pub fn from_transaction(transaction: Transaction) -> Self {
        let Transaction {
            version,
            deps,
            inputs,
            outputs,
            outputs_data,
            witnesses,
            ..
        } = transaction;
        Self {
            version,
            deps,
            inputs,
            outputs,
            outputs_data,
            witnesses,
        }
    }

    pub fn version(mut self, version: u32) -> Self {
        self.version = version;
        self
    }

    pub fn deps(mut self, deps: Deps) -> Self {
        self.deps = deps;
        self
    }

    pub fn cell_dep(mut self, dep: CellDep) -> Self {
        self.deps.cells.push(dep);
        self
    }

    pub fn cell_deps<I, T>(mut self, deps: I) -> Self
    where
        I: IntoIterator<Item = T>,
        T: Into<CellDep>,
    {
        self.deps.cells.extend(deps.into_iter().map(Into::into));
        self
    }

    pub fn cell_deps_clear(mut self) -> Self {
        self.deps.cells.clear();
        self
    }

    pub fn header_dep(mut self, block_hash: H256) -> Self {
        self.deps.headers.push(block_hash);
        self
    }

    pub fn header_deps<I, T>(mut self, block_hashes: I) -> Self
    where
        I: IntoIterator<Item = T>,
        T: Into<H256>,
    {
        self.deps
            .headers
            .extend(block_hashes.into_iter().map(Into::into));
        self
    }

    pub fn header_deps_clear(mut self) -> Self {
        self.deps.headers.clear();
        self
    }

    pub fn input(mut self, input: CellInput) -> Self {
        self.inputs.push(input);
        self
    }

    pub fn inputs<I, T>(mut self, inputs: I) -> Self
    where
        I: IntoIterator<Item = T>,
        T: Into<CellInput>,
    {
        self.inputs.extend(inputs.into_iter().map(Into::into));
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

    pub fn outputs<I, T>(mut self, outputs: I) -> Self
    where
        I: IntoIterator<Item = T>,
        T: Into<CellOutput>,
    {
        self.outputs.extend(outputs.into_iter().map(Into::into));
        self
    }

    pub fn outputs_clear(mut self) -> Self {
        self.outputs.clear();
        self
    }

    pub fn output_data(mut self, data: Bytes) -> Self {
        self.outputs_data.push(data);
        self
    }

    pub fn outputs_data<I, T>(mut self, outputs_data: I) -> Self
    where
        I: IntoIterator<Item = T>,
        T: Into<Bytes>,
    {
        self.outputs_data
            .extend(outputs_data.into_iter().map(Into::into));
        self
    }

    pub fn outputs_data_clear(mut self) -> Self {
        self.outputs_data.clear();
        self
    }

    pub fn witness(mut self, witness: Witness) -> Self {
        self.witnesses.push(witness);
        self
    }

    pub fn witnesses<I, T>(mut self, witnesses: I) -> Self
    where
        I: IntoIterator<Item = T>,
        T: Into<Witness>,
    {
        self.witnesses.extend(witnesses.into_iter().map(Into::into));
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
            outputs_data,
            witnesses,
        } = self;
        Transaction::new(version, deps, inputs, outputs, outputs_data, witnesses)
    }

    /// # Warning
    ///
    /// When using this method, the caller should ensure the input hashes is right, or the caller
    /// will get a incorrect Transaction.
    pub unsafe fn build_unchecked(self, hash: H256, witness_hash: H256) -> Transaction {
        let Self {
            version,
            deps,
            inputs,
            outputs,
            outputs_data,
            witnesses,
        } = self;
        Transaction {
            version,
            deps,
            inputs,
            outputs,
            outputs_data,
            witnesses,
            hash,
            witness_hash,
        }
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
        let mut inner = [0u8; 10];
        inner.copy_from_slice(&h.as_bytes()[..10]);
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
    use crate::{capacity_bytes, script::ScriptHashType, Bytes, Capacity};

    #[test]
    fn tx_hash() {
        let data = Bytes::from(vec![1, 2, 3]);
        let tx = TransactionBuilder::default()
            .output(CellOutput::new(
                capacity_bytes!(5000),
                CellOutput::calculate_data_hash(&data),
                Script::default(),
                None,
            ))
            .output_data(data)
            .input(CellInput::new(OutPoint::new(H256::zero(), 0), 0))
            .witness(vec![Bytes::from(vec![7, 8, 9])])
            .build();

        assert_eq!(
            format!("{:x}", tx.hash()),
            "846078e6efcbdeb61f3aabc2d72963a5211820f0c1b159bfc744c6dc32fc84d0"
        );
        assert_eq!(
            format!("{:x}", tx.witness_hash()),
            "a507f50d0e2bbe790932f33b2e77b2e051ef18d25da0e791f359b4066eb7dc94"
        );
    }

    #[test]
    fn min_cell_output_capacity() {
        let lock = Script::new(vec![], H256::default(), ScriptHashType::Data);
        let output = CellOutput::new(Capacity::zero(), Default::default(), lock, None);
        assert_eq!(
            output.occupied_capacity(Capacity::zero()).unwrap(),
            capacity_bytes!(41)
        );
    }

    #[test]
    fn min_secp256k1_cell_output_capacity() {
        let lock = Script::new(
            vec![vec![0u8; 20].into()],
            H256::default(),
            ScriptHashType::Data,
        );
        let output = CellOutput::new(Capacity::zero(), Default::default(), lock, None);
        assert_eq!(
            output.occupied_capacity(Capacity::zero()).unwrap(),
            capacity_bytes!(61)
        );
    }
}
