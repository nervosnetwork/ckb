pub struct Transaction {
    version: Version,
    cell_deps: Vec<CellDep>,
    header_deps: Vec<H256>,
    inputs: Vec<CellInput>,
    outputs: Vec<CellOutput>,
    witnesses: Vec<Vec<Bytes>>,
}

pub struct CellDep {
    previous_output: OutPoint,
    is_group: bool,
}

pub struct OutPoint {
    tx_hash: H256,
    index: u32,
}

pub enum ScriptHashType {
    Data = 0,
    Type = 1,
}

pub struct Script {
    args: Vec<Bytes>,
    code_hash: H256,
    hash_type: ScriptHashType,
}

pub struct CellInput {
    previous_output: OutPoint,
    lock_script: Script,
    since: u64,
}

pub struct CellOutput {
    properties: CellProperties,
    data: H256,
}

pub struct CellProperties {
    capacity: Capacity,
    lock_hash: H256,
    type_script: Option<Script>,
}
