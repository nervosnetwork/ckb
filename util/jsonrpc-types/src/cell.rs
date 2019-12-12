use crate::{Capacity, CellOutput, JsonBytes, OutPoint, Script, Uint64};
use ckb_types::{
    core::cell::{CellMeta, CellStatus},
    prelude::Unpack,
    H256,
};
use serde::{Deserialize, Serialize};

// This is used as return value of get_cells_by_lock_hash RPC:
// it contains both OutPoint data used for referencing a cell, as well as
// cell's own data such as lock and capacity
#[derive(Debug, Serialize, Deserialize)]
pub struct CellOutputWithOutPoint {
    pub out_point: OutPoint,
    pub block_hash: H256,
    pub capacity: Capacity,
    pub lock: Script,
    #[serde(rename = "type")]
    pub type_: Option<Script>,
    pub output_data_len: Uint64,
    pub cellbase: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CellWithStatus {
    pub cell: Option<CellInfo>,
    pub status: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CellInfo {
    pub output: CellOutput,
    pub data: Option<CellData>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CellData {
    pub content: JsonBytes,
    pub hash: H256,
}

impl From<CellMeta> for CellInfo {
    fn from(cell_meta: CellMeta) -> Self {
        CellInfo {
            output: cell_meta.cell_output.into(),
            data: cell_meta.mem_cell_data.map(|(data, hash)| CellData {
                content: JsonBytes::from_bytes(data),
                hash: hash.unpack(),
            }),
        }
    }
}

impl From<CellStatus> for CellWithStatus {
    fn from(status: CellStatus) -> Self {
        let (cell, status) = match status {
            CellStatus::Live(cell_meta) => (Some(cell_meta), "live"),
            CellStatus::Dead => (None, "dead"),
            CellStatus::Unknown => (None, "unknown"),
        };
        Self {
            cell: cell.map(|cell_meta| (*cell_meta).into()),
            status: status.to_string(),
        }
    }
}
