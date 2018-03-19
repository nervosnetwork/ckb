//! Transaction using Cell.
//! It is similar to Bitcoin Tx <https://en.bitcoin.it/wiki/Protocol_documentation#tx/>
use bigint::H256;
use bincode::serialize;
use hash::sha3_256;

#[derive(Clone, Serialize, Deserialize, PartialEq, Debug)]
pub struct OutPoint {
    // Hash of Transaction
    pub hash: H256,
    // Index of cell_operations
    pub index: u32,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Debug)]
pub struct Recipient {
    pub module_id: u32,
    pub lock: Vec<u8>,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Debug)]
pub struct CellInput {
    pub previous_output: OutPoint,
    // Depends on whether the operation is Transform or Destroy, this is the proof to transform
    // lock or destroy lock.
    pub unlock: Vec<u8>,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Debug)]
pub struct CellOutput {
    pub module: u32,
    pub capacity: u32,
    pub data: Vec<u8>,
    pub lock: Vec<u8>,
    pub recipient: Option<Recipient>,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Debug)]
pub struct CellOperation {
    pub input: Option<CellInput>,
    pub output: Option<CellOutput>,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Debug)]
pub struct Transaction {
    pub version: u32,
    pub cell_groups: Vec<Vec<CellOperation>>,
}

impl Transaction {
    pub fn validate(&self) -> bool {
        // TODO implement it
        true
    }

    pub fn hash(&self) -> H256 {
        sha3_256(serialize(self).unwrap()).into()
    }
}
