use crate::{Capacity, CellOutput, JsonBytes, OutPoint, Script, Uint64};
use ckb_types::{
    core::cell::{CellMeta, CellStatus},
    prelude::Unpack,
    H256,
};
use serde::{Deserialize, Serialize};

/// This is used as return value of `get_cells_by_lock_hash` RPC.
///
/// It contains both OutPoint data used for referencing a cell, as well as
/// the cell's properties such as lock and capacity.
///
/// ## Examples
///
/// ```json
/// # serde_json::from_str::<ckb_jsonrpc_types::CellOutputWithOutPoint>(r#"
/// {
///   "block_hash": "0xf293d02ce5e101b160912aaf15b1b87517b7a6d572c13af9ae4101c1143b22ad",
///   "capacity": "0x2ca86f2642",
///   "cellbase": true,
///   "lock": {
///     "args": "0x",
///     "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
///     "hash_type": "data"
///   },
///   "out_point": {
///     "index": "0x0",
///     "tx_hash": "0xa510932a80fda15a774203404453c5f9c0e8582f11c40f8ce5396f2460f8ccbf"
///   },
///   "output_data_len": "0x0",
///   "type": null
/// }
/// # "#).unwrap();
/// ```
#[derive(Debug, Serialize, Deserialize)]
pub struct CellOutputWithOutPoint {
    /// Reference to a cell via transaction hash and output index.
    pub out_point: OutPoint,
    /// The block hash of the block which committed the transaction.
    pub block_hash: H256,
    /// The cell capacity.
    ///
    /// The capacity of a cell is the value of the cell in Shannons. It is also the upper limit of
    /// the cell occupied storage size where every 100,000,000 Shannons give 1-byte storage.
    pub capacity: Capacity,
    /// The lock script.
    pub lock: Script,
    /// The optional type script.
    ///
    /// The JSON field name is "type".
    #[serde(rename = "type")]
    pub type_: Option<Script>,
    /// The bytes count of the cell data.
    pub output_data_len: Uint64,
    /// Whether this is a cellbase transaction output.
    ///
    /// The cellbase transaction is the first transaction in a block which issues rewards and fees
    /// to miners.
    ///
    /// The cellbase transaction has a maturity period of 4 epochs. Its output cells can only be
    /// used as inputs after 4 epochs.
    pub cellbase: bool,
}

/// The JSON view of a cell with its status information.
///
/// ## Examples
///
/// ```
/// # serde_json::from_str::<ckb_jsonrpc_types::CellWithStatus>(r#"
/// {
///   "cell": {
///     "data": {
///       "content": "0x7f454c460201010000000000000000000200f3000100000078000100000000004000000000000000980000000000000005000000400038000100400003000200010000000500000000000000000000000000010000000000000001000000000082000000000000008200000000000000001000000000000001459308d00573000000002e7368737472746162002e74657874000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000b000000010000000600000000000000780001000000000078000000000000000a0000000000000000000000000000000200000000000000000000000000000001000000030000000000000000000000000000000000000082000000000000001100000000000000000000000000000001000000000000000000000000000000",
///       "hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5"
///     },
///     "output": {
///       "capacity": "0x802665800",
///       "lock": {
///         "args": "0x",
///         "code_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
///         "hash_type": "data"
///       },
///       "type": null
///     }
///   },
///   "status": "live"
/// }
/// # "#).unwrap();
/// ```
///
/// ```
/// # serde_json::from_str::<ckb_jsonrpc_types::CellWithStatus>(r#"
/// {
///   "cell": null,
///   "status": "unknown"
/// }
/// # "#).unwrap();
/// ```
#[derive(Debug, Serialize, Deserialize)]
pub struct CellWithStatus {
    /// The cell information.
    ///
    /// For performance issues, CKB only keeps the information for live cells.
    pub cell: Option<CellInfo>,
    /// Status of the cell.
    ///
    /// Allowed values: "live", "dead", "unknown".
    ///
    /// * `live` - The transaction creating this cell is in the chain, and there are no
    /// transactions found in the chain that uses this cell as an input.
    /// * `dead` - (**Deprecated**: the dead status will be removed since 0.36.0, please do not
    /// rely on the logic that differentiates dead and unknown cells.) The transaction creating
    /// this cell is in the chain, and a transaction is found in the chain which uses this cell as
    /// an input.
    /// * `unknown` - CKB does not know the status of the cell. Either the transaction creating
    /// this cell is not in the chain yet, or it is no longer live.
    pub status: String,
}

/// The JSON view of a cell combining the fields in cell output and cell data.
///
/// ## Examples
///
/// ```
/// # serde_json::from_str::<ckb_jsonrpc_types::CellInfo>(r#"
/// {
///   "data": {
///     "content": "0x7f454c460201010000000000000000000200f3000100000078000100000000004000000000000000980000000000000005000000400038000100400003000200010000000500000000000000000000000000010000000000000001000000000082000000000000008200000000000000001000000000000001459308d00573000000002e7368737472746162002e74657874000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000b000000010000000600000000000000780001000000000078000000000000000a0000000000000000000000000000000200000000000000000000000000000001000000030000000000000000000000000000000000000082000000000000001100000000000000000000000000000001000000000000000000000000000000",
///     "hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5"
///   },
///   "output": {
///     "capacity": "0x802665800",
///     "lock": {
///       "args": "0x",
///       "code_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
///       "hash_type": "data"
///     },
///     "type": null
///   }
/// }
/// # "#).unwrap();
/// ```
#[derive(Debug, Serialize, Deserialize)]
pub struct CellInfo {
    /// Cell fields appears in the transaction `outputs` array.
    pub output: CellOutput,
    /// Cell data.
    ///
    /// This is `null` when the data is not requested, which does not mean the cell data is empty.
    pub data: Option<CellData>,
}

/// The cell data content and hash.
///
/// ## Examples
///
/// ```
/// # serde_json::from_str::<ckb_jsonrpc_types::CellData>(r#"
/// {
///   "content": "0x7f454c460201010000000000000000000200f3000100000078000100000000004000000000000000980000000000000005000000400038000100400003000200010000000500000000000000000000000000010000000000000001000000000082000000000000008200000000000000001000000000000001459308d00573000000002e7368737472746162002e74657874000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000b000000010000000600000000000000780001000000000078000000000000000a0000000000000000000000000000000200000000000000000000000000000001000000030000000000000000000000000000000000000082000000000000001100000000000000000000000000000001000000000000000000000000000000",
///   "hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5"
/// }
/// # "#).unwrap();
/// ```
#[derive(Debug, Serialize, Deserialize)]
pub struct CellData {
    /// Cell content.
    pub content: JsonBytes,
    /// Cell content hash.
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
            cell: cell.map(Into::into),
            status: status.to_string(),
        }
    }
}
