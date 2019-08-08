pub struct Transaction {
    version: Version,

    deps: Vec<Dep>,
    loadable_headers: Vec<H256>,

    // Each input has a corresponding Vec<Bytes> in witnesses
    inputs: Vec<Input>,
    witnesses: Vec<Vec<Bytes>>,

    // Each cell has a corresponding Kernel and Bytes in output_kernels and output_data
    output_kernels: Vec<Kernel>,
    output_data: Vec<Bytes>,
}

pub struct Dep {
    out_point: OutPoint,
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

pub struct Input {
    out_point: OutPoint,
    since: u64,
}

pub struct Kernel {
    capacity: Capacity,
    lock: Script,
    type_: Option<Script>,
}
