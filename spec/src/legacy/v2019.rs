//! Legacy CKB Chain Specification (Edition 2019)

use ckb_jsonrpc_types as rpc;
use ckb_pow::Pow;
use ckb_types::{core, packed, H256, U128};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct ChainSpec {
    name: String,
    pub(crate) genesis: Genesis,
    #[serde(default)]
    params: crate::Params,
    pow: Pow,
    #[serde(skip)]
    pub(crate) hash: packed::Byte32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Genesis {
    version: u32,
    parent_hash: H256,
    timestamp: u64,
    compact_target: u32,
    uncles_hash: H256,
    hash: Option<H256>,
    nonce: U128,
    issued_cells: Vec<IssuedCell>,
    genesis_cell: GenesisCell,
    pub(crate) system_cells: Vec<crate::SystemCell>,
    system_cells_lock: Script,
    bootstrap_lock: Script,
    dep_groups: Vec<crate::DepGroupResource>,
    #[serde(default)]
    satoshi_gift: crate::SatoshiGift,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Script {
    code_hash: H256,
    hash_type: rpc::ScriptHashTypeKind,
    args: rpc::JsonBytes,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct IssuedCell {
    capacity: core::Capacity,
    lock: Script,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct GenesisCell {
    message: String,
    lock: Script,
}

impl From<Script> for rpc::Script {
    fn from(input: Script) -> Self {
        let Script {
            code_hash,
            hash_type: hash_type_kind,
            args,
        } = input;
        let hash_type = match hash_type_kind {
            rpc::ScriptHashTypeKind::Data => rpc::ScriptHashType::Data { vm_version: 0 },
            rpc::ScriptHashTypeKind::Type => rpc::ScriptHashType::Type,
        };
        Self {
            code_hash,
            hash_type,
            args,
        }
    }
}

impl From<IssuedCell> for crate::IssuedCell {
    fn from(input: IssuedCell) -> Self {
        let IssuedCell { capacity, lock } = input;
        Self {
            capacity,
            lock: lock.into(),
        }
    }
}

impl From<GenesisCell> for crate::GenesisCell {
    fn from(input: GenesisCell) -> Self {
        let GenesisCell { message, lock } = input;
        Self {
            message,
            lock: lock.into(),
        }
    }
}

impl From<Genesis> for crate::Genesis {
    fn from(input: Genesis) -> Self {
        let Genesis {
            version,
            parent_hash,
            timestamp,
            compact_target,
            uncles_hash,
            hash,
            nonce,
            issued_cells,
            genesis_cell,
            system_cells,
            system_cells_lock,
            bootstrap_lock,
            dep_groups,
            satoshi_gift,
        } = input;
        Self {
            version,
            parent_hash,
            timestamp,
            compact_target,
            uncles_hash,
            hash,
            nonce,
            issued_cells: issued_cells.into_iter().map(Into::into).collect(),
            genesis_cell: genesis_cell.into(),
            system_cells,
            system_cells_lock: system_cells_lock.into(),
            bootstrap_lock: bootstrap_lock.into(),
            dep_groups,
            satoshi_gift,
        }
    }
}

impl From<ChainSpec> for crate::ChainSpec {
    fn from(input: ChainSpec) -> Self {
        let ChainSpec {
            name,
            genesis,
            params,
            pow,
            hash,
        } = input;
        Self {
            name,
            edition: rpc::ChainEdition::V2021,
            genesis: genesis.into(),
            params,
            pow,
            hash,
            legacy: true,
        }
    }
}
