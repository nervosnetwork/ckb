//! Transaction using Cell.
//! It is similar to Bitcoin Tx <https://en.bitcoin.it/wiki/Protocol_documentation#tx/>
use bigint::H256;
use bincode::serialize;
use hash::sha3_256;
use std::iter::{Chain, Map};
use std::slice;

use error::TxError;

type OutPointsIter<'a> = Chain<
    slice::Iter<'a, OutPoint>,
    Map<slice::Iter<'a, CellInput>, fn(&'a CellInput) -> &'a OutPoint>,
>;

#[derive(Clone, Default, Serialize, Deserialize, Eq, PartialEq, Hash, Debug)]
pub struct OutPoint {
    // Hash of Transaction
    pub hash: H256,
    // Index of cell_operations
    pub index: u32,
}

impl OutPoint {
    pub fn new(hash: H256, index: u32) -> Self {
        OutPoint { hash, index }
    }
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Debug)]
pub struct Recipient {
    pub module: u32,
    pub lock: Vec<u8>,
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Debug)]
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

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Debug)]
pub struct CellOutput {
    pub module: u32,
    pub capacity: u32,
    pub data: Vec<u8>,
    pub lock: Vec<u8>,
    pub recipient: Option<Recipient>,
}

impl CellOutput {
    pub fn new(
        module: u32,
        capacity: u32,
        data: Vec<u8>,
        lock: Vec<u8>,
        recipient: Option<Recipient>,
    ) -> Self {
        CellOutput {
            module,
            capacity,
            data,
            lock,
            recipient,
        }
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Debug, Default)]
pub struct Transaction {
    pub version: u32,
    pub deps: Vec<OutPoint>,
    pub inputs: Vec<CellInput>,
    pub outputs: Vec<CellOutput>,
}

impl CellOutput {
    pub fn bytes_len(&self) -> usize {
        8 + self.data.len() + self.lock.len() + self.recipient.as_ref().map_or(0, |r| r.bytes_len())
    }
}

impl Recipient {
    pub fn bytes_len(&self) -> usize {
        4 + self.lock.len()
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
        sha3_256(serialize(self).unwrap()).into()
    }

    pub fn check_lock(&self, unlock: &[u8], lock: &[u8]) -> bool {
        // TODO: check using pubkey signature
        unlock.is_empty() || !lock.is_empty()
    }

    pub fn out_points_iter(&self) -> OutPointsIter {
        let previous_output_fn: fn(&CellInput) -> &OutPoint = |input| &input.previous_output;
        self.deps
            .iter()
            .chain(self.inputs.iter().map(previous_output_fn))
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
}
