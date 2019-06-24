use super::types::{BlockDeltaInfo, CellIndex, HashType, HeaderInfo, LiveCellInfo, TxInfo};
use crate::{Address, NetworkType};
use ckb_core::{header::Header, script::Script, transaction::CellOutPoint};
use numext_fixed_hash::H256;
use serde_derive::{Deserialize, Serialize};

#[derive(Ord, PartialOrd, Eq, PartialEq, Debug, Hash, Clone, Copy, Serialize, Deserialize)]
#[repr(u16)]
pub enum KeyType {
    // key => value: {type} => {block-hash}
    GenesisHash = 0,
    // key => value: {type} => {NetworkType}
    Network = 1,
    // key => value: {type} => {Header}
    LastHeader = 2,
    // key => value: {type} => u128
    TotalCapacity = 3,

    // >> hash-type: block, transaction, lock, data
    // key => value: {type}:{hash} => {hash-type}
    GlobalHash = 100,
    // key => value: {type}:{tx-hash} => {TxInfo}
    TxMap = 101,
    // key => value: {type}:{Address} => {lock-hash}
    SecpAddrLock = 102,
    // >> Save recent headers for rollback a fork and for statistics
    // key => value: {type}:{block-number} => {HeaderInfo}
    RecentHeader = 103,

    // key => value: {type}:{CellOutPoint} => {LiveCellInfo}
    LiveCellMap = 200,
    // key => value: {type}:{block-number}:{CellIndex} => {CellOutPoint}
    LiveCellIndex = 201,

    // >> Store live cell owned by certain lock
    // key => value: {type}:{lock-hash} => Script
    LockScript = 300,
    // key => value: {type}:{lock-hash} => u64
    LockTotalCapacity = 301,
    // >> NOTE: Remove when capacity changed
    // key => value: {type}:{capacity(u64::MAX - u64)}:{lock-hash} => ()
    LockTotalCapacityIndex = 302,
    // key => value: {type}:{lock-hash}:{block-number}:{CellIndex} => {CellOutPoint}
    LockLiveCellIndex = 303,
    // key => value: {type}:{lock-hash}:{block-number}:{tx-index(u32)} => {tx-hash}
    LockTx = 304,
    // >> for rollback block when fork happen (keep 1000 blocks?)
    // key = value: {type}:{block-number} => {BlockDeltaInfo}
    BlockDelta = 400,
}

impl KeyType {
    pub fn to_bytes(self) -> Vec<u8> {
        (self as u16).to_be_bytes().to_vec()
    }

    pub fn from_bytes(bytes: [u8; 2]) -> KeyType {
        match u16::from_be_bytes(bytes) {
            0 => KeyType::GenesisHash,
            1 => KeyType::Network,
            2 => KeyType::LastHeader,
            3 => KeyType::TotalCapacity,

            100 => KeyType::GlobalHash,
            101 => KeyType::TxMap,
            102 => KeyType::SecpAddrLock,
            103 => KeyType::RecentHeader,

            200 => KeyType::LiveCellMap,
            201 => KeyType::LiveCellIndex,

            300 => KeyType::LockScript,
            301 => KeyType::LockTotalCapacity,
            302 => KeyType::LockTotalCapacityIndex,
            303 => KeyType::LockLiveCellIndex,
            304 => KeyType::LockTx,

            400 => KeyType::BlockDelta,
            value => panic!("Unexpected key type: value={}", value),
        }
    }
}

#[derive(Debug, Clone)]
pub enum Key {
    GenesisHash,
    Network,
    LastHeader,
    TotalCapacity,

    GlobalHash(H256),
    TxMap(H256),
    SecpAddrLock(Address),
    RecentHeader(u64),

    LiveCellMap(CellOutPoint),
    LiveCellIndex(u64, CellIndex),

    LockScript(H256),
    LockTotalCapacity(H256),
    LockTotalCapacityIndex(u64, H256),
    LockLiveCellIndexPrefix(H256, Option<u64>),
    LockLiveCellIndex(H256, u64, CellIndex),
    LockTx(H256, u64, u32),
    BlockDelta(u64),
}

impl Key {
    pub fn to_bytes(&self) -> Vec<u8> {
        match self {
            Key::GenesisHash => KeyType::GenesisHash.to_bytes(),
            Key::Network => KeyType::Network.to_bytes(),
            Key::LastHeader => KeyType::LastHeader.to_bytes(),
            Key::TotalCapacity => KeyType::TotalCapacity.to_bytes(),
            Key::GlobalHash(hash) => {
                let mut bytes = KeyType::GlobalHash.to_bytes();
                bytes.extend(bincode::serialize(hash).unwrap());
                bytes
            }
            Key::TxMap(tx_hash) => {
                let mut bytes = KeyType::TxMap.to_bytes();
                bytes.extend(bincode::serialize(tx_hash).unwrap());
                bytes
            }
            Key::SecpAddrLock(address) => {
                let mut bytes = KeyType::SecpAddrLock.to_bytes();
                bytes.extend(bincode::serialize(address).unwrap());
                bytes
            }
            Key::RecentHeader(number) => {
                let mut bytes = KeyType::RecentHeader.to_bytes();
                bytes.extend(number.to_be_bytes().to_vec());
                bytes
            }
            Key::LiveCellMap(out_point) => {
                let mut bytes = KeyType::LiveCellMap.to_bytes();
                bytes.extend(bincode::serialize(out_point).unwrap());
                bytes
            }
            Key::LiveCellIndex(number, cell_index) => {
                let mut bytes = KeyType::LiveCellIndex.to_bytes();
                // Must use big endian for sort
                bytes.extend(number.to_be_bytes().to_vec());
                bytes.extend(cell_index.to_bytes());
                bytes
            }
            Key::LockScript(lock_hash) => {
                let mut bytes = KeyType::LockScript.to_bytes();
                bytes.extend(bincode::serialize(lock_hash).unwrap());
                bytes
            }
            Key::LockTotalCapacity(lock_hash) => {
                let mut bytes = KeyType::LockTotalCapacity.to_bytes();
                bytes.extend(bincode::serialize(lock_hash).unwrap());
                bytes
            }
            Key::LockTotalCapacityIndex(capacity, lock_hash) => {
                // NOTE: large capacity stay front
                let capacity = std::u64::MAX - capacity;
                let mut bytes = KeyType::LockTotalCapacityIndex.to_bytes();
                bytes.extend(capacity.to_be_bytes().to_vec());
                bytes.extend(bincode::serialize(lock_hash).unwrap());
                bytes
            }
            Key::LockLiveCellIndexPrefix(lock_hash, number_opt) => {
                let mut bytes = KeyType::LockLiveCellIndex.to_bytes();
                bytes.extend(bincode::serialize(lock_hash).unwrap());
                if let Some(number) = number_opt {
                    bytes.extend(number.to_be_bytes().to_vec());
                }
                bytes
            }
            Key::LockLiveCellIndex(lock_hash, number, cell_index) => {
                let mut bytes = KeyType::LockLiveCellIndex.to_bytes();
                bytes.extend(bincode::serialize(lock_hash).unwrap());
                // Must use big endian for sort
                bytes.extend(number.to_be_bytes().to_vec());
                bytes.extend(cell_index.to_bytes());
                bytes
            }
            Key::LockTx(lock_hash, number, tx_index) => {
                let mut bytes = KeyType::LockTx.to_bytes();
                bytes.extend(bincode::serialize(lock_hash).unwrap());
                // Must use big endian for sort
                bytes.extend(number.to_be_bytes().to_vec());
                bytes.extend(tx_index.to_be_bytes().to_vec());
                bytes
            }
            Key::BlockDelta(number) => {
                let mut bytes = KeyType::BlockDelta.to_bytes();
                bytes.extend(number.to_be_bytes().to_vec());
                bytes
            }
        }
    }

    pub fn from_bytes(bytes: &[u8]) -> Key {
        let type_bytes = [bytes[0], bytes[1]];
        let key_type = KeyType::from_bytes(type_bytes);
        let args_bytes = &bytes[2..];
        match key_type {
            KeyType::GenesisHash => Key::GenesisHash,
            KeyType::Network => Key::Network,
            KeyType::LastHeader => Key::LastHeader,
            KeyType::TotalCapacity => Key::TotalCapacity,
            KeyType::GlobalHash => {
                let hash = bincode::deserialize(args_bytes).unwrap();
                Key::GlobalHash(hash)
            }
            KeyType::TxMap => {
                let tx_hash = bincode::deserialize(args_bytes).unwrap();
                Key::TxMap(tx_hash)
            }
            KeyType::SecpAddrLock => {
                let address = bincode::deserialize(args_bytes).unwrap();
                Key::SecpAddrLock(address)
            }
            KeyType::RecentHeader => {
                assert_eq!(args_bytes.len(), 8);
                let mut number_bytes = [0u8; 8];
                number_bytes.copy_from_slice(&args_bytes[..8]);
                let number = u64::from_be_bytes(number_bytes);
                Key::RecentHeader(number)
            }
            KeyType::LiveCellMap => {
                let out_point = bincode::deserialize(args_bytes).unwrap();
                Key::LiveCellMap(out_point)
            }
            KeyType::LiveCellIndex => {
                let mut number_bytes = [0u8; 8];
                let mut cell_index_bytes = [0u8; 8];
                number_bytes.copy_from_slice(&args_bytes[..8]);
                cell_index_bytes.copy_from_slice(&args_bytes[8..]);
                let number = u64::from_be_bytes(number_bytes);
                let cell_index = CellIndex::from_bytes(cell_index_bytes);
                Key::LiveCellIndex(number, cell_index)
            }
            KeyType::LockScript => {
                let lock_hash = bincode::deserialize(args_bytes).unwrap();
                Key::LockScript(lock_hash)
            }
            KeyType::LockTotalCapacity => {
                let lock_hash = bincode::deserialize(args_bytes).unwrap();
                Key::LockTotalCapacity(lock_hash)
            }
            KeyType::LockTotalCapacityIndex => {
                let mut capacity_bytes = [0u8; 8];
                capacity_bytes.copy_from_slice(&args_bytes[..8]);
                let lock_hash_bytes = &args_bytes[8..];
                // NOTE: large capacity stay front
                let capacity = std::u64::MAX - u64::from_be_bytes(capacity_bytes);
                let lock_hash = bincode::deserialize(lock_hash_bytes).unwrap();
                Key::LockTotalCapacityIndex(capacity, lock_hash)
            }
            KeyType::LockLiveCellIndex => {
                let lock_hash_bytes = &args_bytes[..32];
                let mut number_bytes = [0u8; 8];
                number_bytes.copy_from_slice(&args_bytes[32..40]);
                let mut cell_index_bytes = [0u8; 8];
                cell_index_bytes.copy_from_slice(&args_bytes[40..]);
                let lock_hash = bincode::deserialize(lock_hash_bytes).unwrap();
                let number = u64::from_be_bytes(number_bytes);
                let cell_index = CellIndex::from_bytes(cell_index_bytes);
                Key::LockLiveCellIndex(lock_hash, number, cell_index)
            }
            KeyType::LockTx => {
                let lock_hash_bytes = &args_bytes[..32];
                let mut number_bytes = [0u8; 8];
                let mut tx_index_bytes = [0u8; 4];
                number_bytes.copy_from_slice(&args_bytes[32..40]);
                tx_index_bytes.copy_from_slice(&args_bytes[40..]);
                let lock_hash = bincode::deserialize(lock_hash_bytes).unwrap();
                let number = u64::from_be_bytes(number_bytes);
                let tx_index = u32::from_be_bytes(tx_index_bytes);
                Key::LockTx(lock_hash, number, tx_index)
            }
            KeyType::BlockDelta => {
                let mut number_bytes = [0u8; 8];
                number_bytes.copy_from_slice(args_bytes);
                let number = u64::from_be_bytes(number_bytes);
                Key::BlockDelta(number)
            }
        }
    }

    pub fn key_type(&self) -> KeyType {
        match self {
            Key::GenesisHash => KeyType::GenesisHash,
            Key::Network => KeyType::Network,
            Key::LastHeader => KeyType::LastHeader,
            Key::TotalCapacity => KeyType::TotalCapacity,
            Key::GlobalHash(..) => KeyType::GlobalHash,
            Key::TxMap(..) => KeyType::TxMap,
            Key::SecpAddrLock(..) => KeyType::SecpAddrLock,
            Key::RecentHeader(..) => KeyType::RecentHeader,
            Key::LiveCellMap(..) => KeyType::LiveCellMap,
            Key::LiveCellIndex(..) => KeyType::LiveCellIndex,
            Key::LockScript(..) => KeyType::LockScript,
            Key::LockTotalCapacity(..) => KeyType::LockTotalCapacity,
            Key::LockTotalCapacityIndex(..) => KeyType::LockTotalCapacityIndex,
            // Key::LockLiveCell(..) => KeyType::LockLiveCell,
            Key::LockLiveCellIndexPrefix(..) => KeyType::LockLiveCellIndex,
            Key::LockLiveCellIndex(..) => KeyType::LockLiveCellIndex,
            Key::LockTx(..) => KeyType::LockTx,
            Key::BlockDelta(..) => KeyType::BlockDelta,
        }
    }

    pub(crate) fn pair_genesis_hash(value: &H256) -> (Vec<u8>, Vec<u8>) {
        (
            Key::GenesisHash.to_bytes(),
            bincode::serialize(value).unwrap(),
        )
    }
    pub(crate) fn pair_network(value: NetworkType) -> (Vec<u8>, Vec<u8>) {
        (Key::Network.to_bytes(), bincode::serialize(&value).unwrap())
    }
    pub(crate) fn pair_last_header(value: &Header) -> (Vec<u8>, Vec<u8>) {
        (
            Key::LastHeader.to_bytes(),
            bincode::serialize(value).unwrap(),
        )
    }
    pub(crate) fn pair_total_capacity(value: &u128) -> (Vec<u8>, Vec<u8>) {
        (
            Key::TotalCapacity.to_bytes(),
            bincode::serialize(value).unwrap(),
        )
    }

    pub(crate) fn pair_global_hash(hash: H256, value: HashType) -> (Vec<u8>, Vec<u8>) {
        (
            Key::GlobalHash(hash).to_bytes(),
            bincode::serialize(&value).unwrap(),
        )
    }
    pub(crate) fn pair_tx_map(tx_hash: H256, value: &TxInfo) -> (Vec<u8>, Vec<u8>) {
        (
            Key::TxMap(tx_hash).to_bytes(),
            bincode::serialize(value).unwrap(),
        )
    }
    pub(crate) fn pair_secp_addr_lock(address: Address, value: &H256) -> (Vec<u8>, Vec<u8>) {
        (
            Key::SecpAddrLock(address).to_bytes(),
            bincode::serialize(value).unwrap(),
        )
    }
    pub(crate) fn pair_recent_header(value: &HeaderInfo) -> (Vec<u8>, Vec<u8>) {
        (
            Key::RecentHeader(value.header.number()).to_bytes(),
            bincode::serialize(value).unwrap(),
        )
    }

    pub(crate) fn pair_live_cell_map(
        out_point: CellOutPoint,
        value: &LiveCellInfo,
    ) -> (Vec<u8>, Vec<u8>) {
        (
            Key::LiveCellMap(out_point).to_bytes(),
            bincode::serialize(value).unwrap(),
        )
    }
    pub(crate) fn pair_live_cell_index(
        (number, cell_index): (u64, CellIndex),
        value: &CellOutPoint,
    ) -> (Vec<u8>, Vec<u8>) {
        (
            Key::LiveCellIndex(number, cell_index).to_bytes(),
            bincode::serialize(value).unwrap(),
        )
    }

    pub(crate) fn pair_lock_script(lock_hash: H256, value: &Script) -> (Vec<u8>, Vec<u8>) {
        (
            Key::LockScript(lock_hash).to_bytes(),
            bincode::serialize(value).unwrap(),
        )
    }
    pub(crate) fn pair_lock_total_capacity(lock_hash: H256, value: u64) -> (Vec<u8>, Vec<u8>) {
        (
            Key::LockTotalCapacity(lock_hash).to_bytes(),
            bincode::serialize(&value).unwrap(),
        )
    }
    pub(crate) fn pair_lock_total_capacity_index(
        (capacity, lock_hash): (u64, H256),
    ) -> (Vec<u8>, Vec<u8>) {
        (
            Key::LockTotalCapacityIndex(capacity, lock_hash).to_bytes(),
            [0u8].to_vec(),
        )
    }
    pub(crate) fn pair_lock_live_cell_index(
        (lock_hash, number, cell_index): (H256, u64, CellIndex),
        value: &CellOutPoint,
    ) -> (Vec<u8>, Vec<u8>) {
        (
            Key::LockLiveCellIndex(lock_hash, number, cell_index).to_bytes(),
            bincode::serialize(value).unwrap(),
        )
    }
    pub(crate) fn pair_lock_tx(
        (lock_hash, number, tx_index): (H256, u64, u32),
        value: &H256,
    ) -> (Vec<u8>, Vec<u8>) {
        (
            Key::LockTx(lock_hash, number, tx_index).to_bytes(),
            bincode::serialize(value).unwrap(),
        )
    }

    pub(crate) fn pair_block_delta(value: &BlockDeltaInfo) -> (Vec<u8>, Vec<u8>) {
        let number = value.number();
        (
            Key::BlockDelta(number).to_bytes(),
            bincode::serialize(value).unwrap(),
        )
    }
}

#[derive(Eq, PartialEq, Debug, Clone, Serialize, Deserialize, Default)]
pub struct KeyMetrics {
    count: usize,
    key_size: usize,
    value_size: usize,
    total_size: usize,
}

impl KeyMetrics {
    pub fn add_pair(&mut self, key: &[u8], value: &[u8]) {
        self.count += 1;
        self.key_size += key.len();
        self.value_size += value.len();
        self.total_size += key.len() + value.len();
    }
}
