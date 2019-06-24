use std::collections::{HashMap, HashSet};

use ckb_core::{
    block::Block,
    header::Header,
    script::Script,
    transaction::{CellInput, CellOutPoint, OutPoint},
};
use numext_fixed_hash::H256;
use serde_derive::{Deserialize, Serialize};

use super::key::{Key, KeyType};
use super::util::{put_pair, value_to_bytes};
use crate::Address;

const KEEP_RECENT_HEADERS: u64 = 10_000;
const KEEP_RECENT_BLOCKS: u64 = 200;

#[derive(Hash, Eq, PartialEq, Debug, Clone, Copy, Serialize, Deserialize)]
#[repr(u8)]
pub enum HashType {
    Block,
    Transaction,
    Lock,
    Data,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockDeltaInfo {
    pub(crate) header_info: HeaderInfo,
    pub(crate) parent_header: Option<Header>,
    txs: Vec<RichTxInfo>,
    locks: Vec<(H256, LockInfo)>,
    old_headers: Vec<u64>,
    old_blocks: Vec<u64>,
    old_chain_capacity: u128,
    new_chain_capacity: u128,
}

impl BlockDeltaInfo {
    pub(crate) fn hash(&self) -> &H256 {
        self.header_info.header.hash()
    }
    pub(crate) fn number(&self) -> u64 {
        self.header_info.header.number()
    }

    pub(crate) fn from_block(
        block: &Block,
        store: rkv::SingleStore,
        writer: &rkv::Writer,
        secp_code_hash: &H256,
    ) -> BlockDeltaInfo {
        let block_header: Header = block.header().clone();
        let block_number = block_header.number();
        let timestamp = block_header.timestamp();

        // Collect old headers to be deleted
        let mut old_headers = Vec::new();
        let mut old_blocks = Vec::new();
        for item in store
            .iter_from(writer, &KeyType::RecentHeader.to_bytes())
            .unwrap()
        {
            let (key_bytes, _) = item.unwrap();
            if let Key::RecentHeader(number) = Key::from_bytes(key_bytes) {
                if number + KEEP_RECENT_HEADERS <= block_number {
                    old_headers.push(number);
                } else {
                    break;
                }
            } else {
                break;
            }
        }
        for item in store
            .iter_from(writer, &KeyType::BlockDelta.to_bytes())
            .unwrap()
        {
            let (key_bytes, _) = item.unwrap();
            if let Key::BlockDelta(number) = Key::from_bytes(key_bytes) {
                if number + KEEP_RECENT_BLOCKS <= block_number {
                    old_blocks.push(number);
                } else {
                    break;
                }
            } else {
                break;
            }
        }

        let mut cell_removed = 0;
        let mut cell_added = 0;
        let mut locks: HashMap<H256, LockInfo> = HashMap::default();
        let txs = block
            .transactions()
            .iter()
            .enumerate()
            .map(|(tx_index, tx)| {
                let mut inputs = Vec::new();
                let mut outputs = Vec::new();

                for out_point in tx
                    .inputs()
                    .iter()
                    .filter_map(|input| input.previous_output.cell.as_ref())
                {
                    let live_cell_info: LiveCellInfo = store
                        .get(writer, Key::LiveCellMap(out_point.clone()).to_bytes())
                        .unwrap()
                        .as_ref()
                        .map(|value| value_to_bytes(value))
                        .map(|bytes| bincode::deserialize(&bytes).unwrap())
                        .unwrap();
                    let lock_hash = live_cell_info.lock_hash.clone();
                    let capacity = live_cell_info.capacity;
                    inputs.push(live_cell_info);

                    locks
                        .entry(lock_hash.clone())
                        .or_insert_with(move || {
                            let lock_capacity: u64 = store
                                .get(writer, Key::LockTotalCapacity(lock_hash).to_bytes())
                                .unwrap()
                                .map(|value| bincode::deserialize(value_to_bytes(&value)).unwrap())
                                .unwrap_or(0);
                            LockInfo::new(lock_capacity)
                        })
                        .add_input(capacity);
                }

                for (output_index, output) in tx.outputs().iter().enumerate() {
                    let lock: Script = output.lock.clone();
                    let lock_hash = lock.hash();
                    let capacity = output.capacity.as_u64();
                    let out_point = CellOutPoint {
                        tx_hash: tx.hash().clone(),
                        index: output_index as u32,
                    };
                    let cell_index = CellIndex::new(tx_index as u32, output_index as u32);

                    let live_cell_info = LiveCellInfo {
                        out_point,
                        index: cell_index,
                        lock_hash: lock_hash.clone(),
                        capacity,
                        number: block_number,
                    };
                    outputs.push(live_cell_info);

                    let lock_info = locks.entry(lock_hash.clone()).or_insert_with(|| {
                        let lock_capacity: u64 = store
                            .get(writer, Key::LockTotalCapacity(lock_hash).to_bytes())
                            .unwrap()
                            .map(|value| bincode::deserialize(value_to_bytes(&value)).unwrap())
                            .unwrap_or(0);
                        LockInfo::new(lock_capacity)
                    });
                    lock_info.set_script(lock.clone(), secp_code_hash);
                    lock_info.add_output(capacity);
                }

                cell_removed += inputs.len();
                cell_added += outputs.len();
                RichTxInfo {
                    tx_hash: tx.hash().clone(),
                    tx_index: tx_index as u32,
                    block_number,
                    block_timestamp: timestamp,
                    inputs,
                    outputs,
                }
            })
            .collect::<Vec<_>>();

        let locks_old_total: u64 = locks.values().map(|info| info.old_total_capacity).sum();
        let locks_new_total: u64 = locks.values().map(|info| info.new_total_capacity).sum();
        let old_chain_capacity: u128 = store
            .get(writer, Key::TotalCapacity.to_bytes())
            .unwrap()
            .map(|value| bincode::deserialize(value_to_bytes(&value)).unwrap())
            .unwrap_or(0);
        let new_chain_capacity: u128 =
            old_chain_capacity - u128::from(locks_old_total) + u128::from(locks_new_total);

        let capacity_delta = (new_chain_capacity as i128 - old_chain_capacity as i128) as i64;
        let header_info = HeaderInfo {
            header: block_header.clone(),
            txs_size: block.transactions().len() as u32,
            uncles_size: block.uncles().len() as u32,
            proposals_size: block.proposals().len() as u32,
            new_chain_capacity,
            capacity_delta,
            cell_removed: cell_removed as u32,
            cell_added: cell_added as u32,
        };

        let parent_header = if block_number > 0 {
            Some(
                store
                    .get(writer, Key::RecentHeader(block_number - 1).to_bytes())
                    .expect("read recent header failed")
                    .map(|value| {
                        let info: HeaderInfo =
                            bincode::deserialize(value_to_bytes(&value)).unwrap();
                        info.header
                    })
                    .expect("Rollback so many blocks???"),
            )
        } else {
            None
        };
        BlockDeltaInfo {
            header_info,
            parent_header,
            txs,
            locks: locks.into_iter().collect::<Vec<_>>(),
            old_headers,
            old_blocks,
            old_chain_capacity,
            new_chain_capacity,
        }
    }

    pub(crate) fn apply(&self, store: rkv::SingleStore, writer: &mut rkv::Writer) -> ApplyResult {
        log::debug!(
            "apply block: number={}, txs={}, locks={}",
            self.header_info.header.number(),
            self.txs.len(),
            self.locks.len(),
        );

        // Update cells and transactions
        for tx in &self.txs {
            put_pair(
                store,
                writer,
                Key::pair_tx_map(tx.tx_hash.clone(), &tx.to_thin()),
            );

            for LiveCellInfo {
                out_point,
                lock_hash,
                number,
                index,
                ..
            } in &tx.inputs
            {
                put_pair(
                    store,
                    writer,
                    Key::pair_lock_tx((lock_hash.clone(), *number, index.tx_index), &tx.tx_hash),
                );
                store
                    .delete(writer, Key::LiveCellMap(out_point.clone()).to_bytes())
                    .unwrap();
                store
                    .delete(writer, Key::LiveCellIndex(*number, *index).to_bytes())
                    .unwrap();
                store
                    .delete(
                        writer,
                        Key::LockLiveCellIndex(lock_hash.clone(), *number, *index).to_bytes(),
                    )
                    .unwrap();
            }

            for live_cell_info in &tx.outputs {
                let LiveCellInfo {
                    out_point,
                    lock_hash,
                    number,
                    index,
                    ..
                } = live_cell_info;
                put_pair(
                    store,
                    writer,
                    Key::pair_lock_tx((lock_hash.clone(), *number, index.tx_index), &tx.tx_hash),
                );
                put_pair(
                    store,
                    writer,
                    Key::pair_live_cell_map(out_point.clone(), live_cell_info),
                );
                put_pair(
                    store,
                    writer,
                    Key::pair_live_cell_index((*number, *index), out_point),
                );
                put_pair(
                    store,
                    writer,
                    Key::pair_lock_live_cell_index((lock_hash.clone(), *number, *index), out_point),
                );
            }
        }

        for (lock_hash, info) in &self.locks {
            let LockInfo {
                script_opt,
                address_opt,
                old_total_capacity,
                new_total_capacity,
                ..
            } = info;
            put_pair(
                store,
                writer,
                Key::pair_global_hash(lock_hash.clone(), HashType::Lock),
            );
            if let Some(script) = script_opt {
                put_pair(
                    store,
                    writer,
                    Key::pair_lock_script(lock_hash.clone(), script),
                );
            }
            if let Some(address) = address_opt {
                put_pair(
                    store,
                    writer,
                    Key::pair_secp_addr_lock(address.clone(), &lock_hash),
                );
            }

            if old_total_capacity != new_total_capacity {
                // Update lock capacity keys
                if let Err(err) = store.delete(
                    writer,
                    Key::LockTotalCapacityIndex(*old_total_capacity, (*lock_hash).clone())
                        .to_bytes(),
                ) {
                    log::debug!(
                        "Delete LockTotalCapacityIndex({}, {}) error: {:?}",
                        old_total_capacity,
                        lock_hash,
                        err
                    );
                }

                if *new_total_capacity > 0 {
                    put_pair(
                        store,
                        writer,
                        Key::pair_lock_total_capacity((*lock_hash).clone(), *new_total_capacity),
                    );
                    put_pair(
                        store,
                        writer,
                        Key::pair_lock_total_capacity_index((
                            *new_total_capacity,
                            (*lock_hash).clone(),
                        )),
                    );
                } else {
                    store
                        .delete(
                            writer,
                            Key::LockTotalCapacity((*lock_hash).clone()).to_bytes(),
                        )
                        .unwrap();
                }
            }
        }
        // Update total capacity
        put_pair(
            store,
            writer,
            Key::pair_total_capacity(&self.new_chain_capacity),
        );

        // Add recent header
        put_pair(store, writer, Key::pair_recent_header(&self.header_info));
        put_pair(store, writer, Key::pair_block_delta(&self));
        // Clean old header infos
        for old_number in &self.old_headers {
            store
                .delete(writer, Key::RecentHeader(*old_number).to_bytes())
                .unwrap();
        }
        for old_number in &self.old_blocks {
            store
                .delete(writer, Key::BlockDelta(*old_number).to_bytes())
                .unwrap();
        }
        // Update last header
        put_pair(
            store,
            writer,
            Key::pair_last_header(&self.header_info.header),
        );

        self.header_info.clone().into()
    }

    pub(crate) fn rollback(&self, store: rkv::SingleStore, writer: &mut rkv::Writer) {
        log::debug!("rollback block: {:?}", self);

        let mut delete_lock_txs: HashSet<(H256, u64, u32)> = HashSet::default();
        for tx in &self.txs {
            store
                .delete(writer, Key::TxMap(tx.tx_hash.clone()).to_bytes())
                .unwrap();
            for live_cell_info in &tx.inputs {
                let LiveCellInfo {
                    out_point,
                    lock_hash,
                    number,
                    index,
                    ..
                } = live_cell_info;
                delete_lock_txs.insert((lock_hash.clone(), *number, index.tx_index));
                put_pair(
                    store,
                    writer,
                    Key::pair_live_cell_map(out_point.clone(), live_cell_info),
                );
                put_pair(
                    store,
                    writer,
                    Key::pair_live_cell_index((*number, *index), out_point),
                );
                put_pair(
                    store,
                    writer,
                    Key::pair_lock_live_cell_index((lock_hash.clone(), *number, *index), out_point),
                );
            }

            for live_cell_info in &tx.outputs {
                let LiveCellInfo {
                    out_point,
                    lock_hash,
                    number,
                    index,
                    ..
                } = live_cell_info;
                delete_lock_txs.insert((lock_hash.clone(), *number, index.tx_index));
                store
                    .delete(writer, Key::LiveCellMap(out_point.clone()).to_bytes())
                    .unwrap();
                store
                    .delete(writer, Key::LiveCellIndex(*number, *index).to_bytes())
                    .unwrap();
                store
                    .delete(
                        writer,
                        Key::LockLiveCellIndex(lock_hash.clone(), *number, *index).to_bytes(),
                    )
                    .unwrap();
            }
        }
        for (lock_hash, number, tx_index) in delete_lock_txs {
            store
                .delete(writer, Key::LockTx(lock_hash, number, tx_index).to_bytes())
                .unwrap();
        }

        for (lock_hash, info) in &self.locks {
            let LockInfo {
                old_total_capacity,
                new_total_capacity,
                ..
            } = info;

            if old_total_capacity != new_total_capacity {
                // Update lock capacity keys
                if let Err(err) = store.delete(
                    writer,
                    Key::LockTotalCapacityIndex(*new_total_capacity, (*lock_hash).clone())
                        .to_bytes(),
                ) {
                    log::debug!(
                        "Delete LockTotalCapacityIndex({}, {}) error: {:?}",
                        new_total_capacity,
                        lock_hash,
                        err
                    );
                }

                if *old_total_capacity > 0 {
                    put_pair(
                        store,
                        writer,
                        Key::pair_lock_total_capacity((*lock_hash).clone(), *old_total_capacity),
                    );
                    put_pair(
                        store,
                        writer,
                        Key::pair_lock_total_capacity_index((
                            *old_total_capacity,
                            (*lock_hash).clone(),
                        )),
                    );
                } else {
                    store
                        .delete(
                            writer,
                            Key::LockTotalCapacity((*lock_hash).clone()).to_bytes(),
                        )
                        .unwrap();
                }
            }
        }
        // Rollback total capacity
        put_pair(
            store,
            writer,
            Key::pair_total_capacity(&self.old_chain_capacity),
        );
        // Remove recent header
        store
            .delete(writer, Key::RecentHeader(self.number()).to_bytes())
            .unwrap();
        // Remove recent block
        store
            .delete(writer, Key::BlockDelta(self.number()).to_bytes())
            .unwrap();
        // Update last header
        put_pair(
            store,
            writer,
            Key::pair_last_header(self.parent_header.as_ref().unwrap()),
        );
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockInfo {
    script_opt: Option<Script>,
    address_opt: Option<Address>,
    old_total_capacity: u64,
    new_total_capacity: u64,
    inputs_capacity: u64,
    outputs_capacity: u64,
}

impl LockInfo {
    fn new(old_total_capacity: u64) -> LockInfo {
        LockInfo {
            script_opt: None,
            address_opt: None,
            old_total_capacity,
            new_total_capacity: old_total_capacity,
            inputs_capacity: 0,
            outputs_capacity: 0,
        }
    }

    fn set_script(&mut self, script: Script, secp_code_hash: &H256) {
        let address_opt = if &script.code_hash == secp_code_hash {
            if script.args.len() == 1 {
                let lock_arg = &script.args[0];
                match Address::from_lock_arg(&lock_arg) {
                    Ok(address) => Some(address),
                    Err(err) => {
                        log::info!("Invalid secp arg: {:?} => {}", lock_arg, err);
                        None
                    }
                }
            } else {
                log::info!("lock arg should given exact 1");
                None
            }
        } else {
            None
        };
        self.script_opt = Some(script);
        self.address_opt = address_opt;
    }

    fn add_input(&mut self, input_capacity: u64) {
        self.inputs_capacity += input_capacity;
        assert!(self.new_total_capacity >= input_capacity);
        self.new_total_capacity -= input_capacity;
    }

    fn add_output(&mut self, output_capacity: u64) {
        self.outputs_capacity += output_capacity;
        self.new_total_capacity += output_capacity;
    }
}

pub(crate) struct ApplyResult {
    pub chain_capacity: u128,
    pub capacity_delta: i64,
    pub cell_removed: u32,
    pub cell_added: u32,
    pub txs: u32,
}

#[derive(Hash, Eq, PartialEq, Debug, Clone, Serialize, Deserialize)]
pub struct LiveCellInfo {
    pub out_point: CellOutPoint,
    pub lock_hash: H256,
    // Secp256k1 address
    pub capacity: u64,
    // Block number
    pub number: u64,
    // Location in the block
    pub index: CellIndex,
}

impl LiveCellInfo {
    pub fn core_input(&self) -> CellInput {
        CellInput {
            previous_output: OutPoint {
                cell: Some(self.out_point.clone()),
                block_hash: None,
            },
            since: 0,
        }
    }
}

// LiveCell index in a block
#[derive(Debug, Hash, Eq, PartialEq, Clone, Copy, Serialize, Deserialize)]
pub struct CellIndex {
    // The transaction index in the block
    pub tx_index: u32,
    // The output index in the transaction
    pub output_index: u32,
}

impl CellIndex {
    pub(crate) fn to_bytes(self) -> Vec<u8> {
        let mut bytes = self.tx_index.to_be_bytes().to_vec();
        bytes.extend(self.output_index.to_be_bytes().to_vec());
        bytes
    }

    pub(crate) fn from_bytes(bytes: [u8; 8]) -> CellIndex {
        let mut tx_index_bytes = [0u8; 4];
        let mut output_index_bytes = [0u8; 4];
        tx_index_bytes.copy_from_slice(&bytes[..4]);
        output_index_bytes.copy_from_slice(&bytes[4..]);
        CellIndex {
            tx_index: u32::from_be_bytes(tx_index_bytes),
            output_index: u32::from_be_bytes(output_index_bytes),
        }
    }
}

impl CellIndex {
    pub(crate) fn new(tx_index: u32, output_index: u32) -> CellIndex {
        CellIndex {
            tx_index,
            output_index,
        }
    }
}

#[derive(Debug, Eq, PartialEq, Clone, Serialize, Deserialize)]
pub struct HeaderInfo {
    pub header: Header,
    pub txs_size: u32,
    pub uncles_size: u32,
    pub proposals_size: u32,
    pub new_chain_capacity: u128,
    pub capacity_delta: i64,
    pub cell_removed: u32,
    pub cell_added: u32,
}

impl From<HeaderInfo> for ApplyResult {
    fn from(info: HeaderInfo) -> ApplyResult {
        ApplyResult {
            chain_capacity: info.new_chain_capacity,
            capacity_delta: info.capacity_delta,
            txs: info.txs_size,
            cell_removed: info.cell_removed,
            cell_added: info.cell_added,
        }
    }
}

#[derive(Debug, Hash, Eq, PartialEq, Clone, Serialize, Deserialize)]
pub(crate) struct RichTxInfo {
    tx_hash: H256,
    // Transaction index in target block
    tx_index: u32,
    block_number: u64,
    block_timestamp: u64,
    inputs: Vec<LiveCellInfo>,
    outputs: Vec<LiveCellInfo>,
}

impl RichTxInfo {
    pub(crate) fn to_thin(&self) -> TxInfo {
        TxInfo {
            tx_hash: self.tx_hash.clone(),
            tx_index: self.tx_index,
            block_number: self.block_number,
            block_timestamp: self.block_timestamp,
            inputs: self
                .inputs
                .iter()
                .map(|info| info.out_point.clone())
                .collect::<Vec<_>>(),
            outputs: self
                .outputs
                .iter()
                .map(|info| info.out_point.clone())
                .collect::<Vec<_>>(),
        }
    }
}

#[derive(Debug, Hash, Eq, PartialEq, Clone, Serialize, Deserialize)]
pub struct TxInfo {
    pub tx_hash: H256,
    // Transaction index in target block
    pub tx_index: u32,
    pub block_number: u64,
    pub block_timestamp: u64,
    pub inputs: Vec<CellOutPoint>,
    pub outputs: Vec<CellOutPoint>,
}
