//! Transaction using Cell.
//! It is similar to Bitcoin Tx <https://en.bitcoin.it/wiki/Protocol_documentation#tx/>
use bigint::H256;
use bincode::serialize;
use error::TxError;
use hash::sha3_256;
use nervos_protocol;

pub const VERSION: u32 = 0;

#[derive(Clone, Serialize, Deserialize, Eq, PartialEq, Hash, Debug)]
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
    pub unlock: Vec<u8>,
}

impl CellInput {
    pub fn new(previous_output: OutPoint, unlock: Vec<u8>) -> Self {
        CellInput {
            previous_output,
            unlock,
        }
    }
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct CellOutput {
    pub module: u32,
    pub capacity: u32,
    pub data: Vec<u8>,
    pub lock: Vec<u8>,
}

impl CellOutput {
    pub fn new(module: u32, capacity: u32, data: Vec<u8>, lock: Vec<u8>) -> Self {
        CellOutput {
            module,
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
    /// memorise Hash
    #[serde(skip_serializing, skip_deserializing)]
    pub hash: Option<H256>,
}

impl CellOutput {
    pub fn bytes_len(&self) -> usize {
        8 + self.data.len() + self.lock.len()
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
            hash: None,
        }
    }

    pub fn is_cellbase(&self) -> bool {
        self.inputs.len() == 1 && self.inputs[0].previous_output.is_null()
    }

    // TODO: split it
    pub fn validate(&self, is_enlarge_transaction: bool) -> Result<(), TxError> {
        if is_enlarge_transaction && !(self.inputs.is_empty() && self.outputs.len() == 1) {
            return Err(TxError::WrongFormat);
        }

        // check outputs capacity
        for output in &self.outputs {
            if output.bytes_len() > (output.capacity as usize) {
                return Err(TxError::OutofBound);
            }
        }

        Ok(())
    }

    pub fn hash(&self) -> H256 {
        self.hash
            .unwrap_or_else(|| sha3_256(serialize(self).unwrap()).into())
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
}

impl<'a> From<&'a OutPoint> for nervos_protocol::OutPoint {
    fn from(o: &'a OutPoint) -> Self {
        let mut op = nervos_protocol::OutPoint::new();
        op.set_hash(o.hash.to_vec());
        op.set_index(o.index);
        op
    }
}

impl<'a> From<&'a nervos_protocol::OutPoint> for OutPoint {
    fn from(o: &'a nervos_protocol::OutPoint) -> Self {
        Self {
            hash: H256::from(o.get_hash()),
            index: o.get_index(),
        }
    }
}

impl<'a> From<&'a nervos_protocol::CellInput> for CellInput {
    fn from(c: &'a nervos_protocol::CellInput) -> Self {
        Self {
            previous_output: c.get_previous_output().into(),
            unlock: c.get_unlock().to_vec(),
        }
    }
}

impl<'a> From<&'a CellInput> for nervos_protocol::CellInput {
    fn from(c: &'a CellInput) -> Self {
        let mut ci = nervos_protocol::CellInput::new();
        ci.set_previous_output((&c.previous_output).into());
        ci.set_unlock(c.unlock.clone());
        ci
    }
}

impl From<CellInput> for nervos_protocol::CellInput {
    fn from(c: CellInput) -> Self {
        let CellInput {
            previous_output,
            unlock,
        } = c;
        let mut ci = nervos_protocol::CellInput::new();
        ci.set_previous_output((&previous_output).into());
        ci.set_unlock(unlock);
        ci
    }
}

/// stupid proto3
impl<'a> From<&'a nervos_protocol::CellOutput> for CellOutput {
    fn from(c: &'a nervos_protocol::CellOutput) -> Self {
        Self {
            module: c.get_module(),
            capacity: c.get_capacity(),
            data: c.get_data().to_vec(),
            lock: c.get_lock().to_vec(),
        }
    }
}

impl<'a> From<&'a CellOutput> for nervos_protocol::CellOutput {
    fn from(c: &'a CellOutput) -> Self {
        let mut co = nervos_protocol::CellOutput::new();
        co.set_module(c.module);
        co.set_capacity(c.capacity);
        co.set_data(c.data.clone());
        co.set_lock(c.lock.clone());
        co
    }
}

impl From<CellOutput> for nervos_protocol::CellOutput {
    fn from(c: CellOutput) -> Self {
        let CellOutput {
            module,
            capacity,
            data,
            lock,
        } = c;
        let mut co = nervos_protocol::CellOutput::new();
        co.set_module(module);
        co.set_capacity(capacity);
        co.set_data(data);
        co.set_lock(lock);
        co
    }
}

impl<'a> From<&'a nervos_protocol::Transaction> for Transaction {
    fn from(t: &'a nervos_protocol::Transaction) -> Self {
        Self {
            version: t.get_version(),
            deps: t.get_deps().iter().map(Into::into).collect(),
            inputs: t.get_inputs().iter().map(Into::into).collect(),
            outputs: t.get_outputs().iter().map(Into::into).collect(),
            hash: None,
        }
    }
}

impl<'a> From<&'a Transaction> for nervos_protocol::Transaction {
    fn from(t: &'a Transaction) -> Self {
        let mut tx = nervos_protocol::Transaction::new();
        tx.set_version(t.version);
        tx.set_inputs(t.inputs.iter().map(Into::into).collect());
        tx.set_outputs(t.outputs.iter().map(Into::into).collect());
        tx
    }
}
