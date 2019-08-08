pub struct Transaction {
    version: Version,
    deps: Vec<Dep>,
    loadable_headers: Vec<H256>,
    inputs: Vec<Input>,
    outputs: Vec<Output>,
    witnesses: Vec<Vec<Bytes>>,
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

pub struct Output {
    kernel: Kernel,
    data: Bytes,
}

pub struct Kernel {
    capacity: Capacity,
    lock: Script,
    type_: Option<Script>,
}
