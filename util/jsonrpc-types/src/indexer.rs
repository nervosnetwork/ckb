use crate::{BlockNumber, Capacity, CellOutput, JsonBytes, OutPoint, Script, Uint32, Uint64};
use ckb_types::H256;
use serde::{Deserialize, Serialize};

/// Indexer tip information
#[derive(Serialize)]
pub struct IndexerTip {
    /// indexed tip block hash
    pub block_hash: H256,
    /// indexed tip block number
    pub block_number: BlockNumber,
}

/// Live cell
#[derive(Serialize)]
pub struct IndexerCell {
    /// the fields of an output cell
    pub output: CellOutput,
    /// the cell data
    pub output_data: Option<JsonBytes>,
    /// reference to a cell via transaction hash and output index
    pub out_point: OutPoint,
    /// the number of the transaction committed in the block
    pub block_number: BlockNumber,
    /// the position index of the transaction committed in the block
    pub tx_index: Uint32,
}

/// IndexerPagination wraps objects array and last_cursor to provide paging
#[derive(Serialize)]
pub struct IndexerPagination<T> {
    /// objects collection
    pub objects: Vec<T>,
    /// pagination parameter
    pub last_cursor: JsonBytes,
}

impl<T> IndexerPagination<T> {
    /// Construct new IndexerPagination
    pub fn new(objects: Vec<T>, last_cursor: JsonBytes) -> Self {
        IndexerPagination {
            objects,
            last_cursor,
        }
    }
}

/// SearchKey represent indexer support params
#[derive(Deserialize)]
pub struct IndexerSearchKey {
    /// Script
    pub script: Script,
    /// Script Type
    pub script_type: IndexerScriptType,
    /// Script search mode, optional default is `prefix`, means search script with prefix
    pub script_search_mode: Option<IndexerScriptSearchMode>,
    /// filter cells by following conditions, all conditions are optional
    pub filter: Option<IndexerSearchKeyFilter>,
    /// bool, optional default is `true`, if with_data is set to false, the field of returning cell.output_data is null in the result
    pub with_data: Option<bool>,
    /// bool, optional default is `false`, if group_by_transaction is set to true, the returning objects will be grouped by the tx hash
    pub group_by_transaction: Option<bool>,
}

impl Default for IndexerSearchKey {
    fn default() -> Self {
        Self {
            script: Script::default(),
            script_type: IndexerScriptType::Lock,
            script_search_mode: None,
            filter: None,
            with_data: None,
            group_by_transaction: None,
        }
    }
}

/// IndexerScriptSearchMode represent script search mode, default is prefix search
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IndexerScriptSearchMode {
    /// Mode `prefix` search script with prefix
    Prefix,
    /// Mode `exact` search script with exact match
    Exact,
}

impl Default for IndexerScriptSearchMode {
    fn default() -> Self {
        Self::Prefix
    }
}

/// A array represent (half-open) range bounded inclusively below and exclusively above [start, end).
///
/// ## Examples
///
/// |            JSON          |            range             |
/// | -------------------------| ---------------------------- |
/// | ["0x0", "0x2"]           |          [0, 2)              |
/// | ["0x0", "0x174876e801"]  |          [0, 100000000001)   |
///
#[derive(Deserialize, Default)]
#[serde(transparent)]
pub struct IndexerRange {
    inner: [Uint64; 2],
}

impl IndexerRange {
    /// Construct new range
    pub fn new<U>(start: U, end: U) -> Self
    where
        U: Into<Uint64>,
    {
        IndexerRange {
            inner: [start.into(), end.into()],
        }
    }

    /// Return the lower bound of the range (inclusive).
    pub fn start(&self) -> Uint64 {
        self.inner[0]
    }

    /// Return the upper bound of the range (exclusive).
    pub fn end(&self) -> Uint64 {
        self.inner[1]
    }
}

/// IndexerSearchKeyFilter represent indexer params `filter`
#[derive(Deserialize, Default)]
pub struct IndexerSearchKeyFilter {
    /// if search script type is lock, filter cells by type script prefix, and vice versa
    pub script: Option<Script>,
    /// filter cells by script len range
    pub script_len_range: Option<IndexerRange>,
    /// filter cells by output data len range
    pub output_data_len_range: Option<IndexerRange>,
    /// filter cells by output capacity range
    pub output_capacity_range: Option<IndexerRange>,
    /// filter cells by block number range
    pub block_range: Option<IndexerRange>,
}

/// ScriptType `Lock` | `Type`
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IndexerScriptType {
    /// Lock
    Lock,
    /// Type
    Type,
}

/// Order Desc | Asc
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IndexerOrder {
    /// Descending order
    Desc,
    /// Ascending order
    Asc,
}

/// Cells capacity
#[derive(Serialize)]
pub struct IndexerCellsCapacity {
    /// total capacity
    pub capacity: Capacity,
    /// indexed tip block hash
    pub block_hash: H256,
    /// indexed tip block number
    pub block_number: BlockNumber,
}

/// Indexer Transaction Object
#[derive(Serialize)]
#[serde(untagged)]
pub enum IndexerTx {
    /// # Ungrouped format represent as `IndexerTxWithCell`
    ///
    /// ## Fields
    ///
    /// `IndexerCellType` is equivalent to `"input" | "output"`.
    ///
    /// `IndexerTxWithCell` is a JSON object with the following fields.
    /// *   `tx_hash`: [`H256`] - transaction hash
    /// *   `block_number`: [`BlockNumber`] - the number of the transaction committed in the block
    /// *   `tx_index`: [`Uint32`] - the position index of the transaction committed in the block
    /// *   `io_index`: [`Uint32`] - the position index of the cell in the transaction inputs or outputs
    /// *   `io_type`: [`IndexerCellType`] - io type
    ///
    Ungrouped(IndexerTxWithCell),
    /// # Grouped format represent as `IndexerTxWithCells`
    ///
    /// ## Fields
    ///
    /// `IndexerCellType` is equivalent to `"input" | "output"`.
    ///
    /// `IndexerTxWithCells` is a JSON object with the following fields.
    /// *   `tx_hash`: [`H256`] - transaction hash
    /// *   `block_number`: [`BlockNumber`] - the number of the transaction committed in the block
    /// *   `tx_index`: [`Uint32`]- the position index of the transaction committed in the block
    /// *   `cells`: Array <(IndexerCellType, Uint32)>
    ///
    Grouped(IndexerTxWithCells),
}

impl IndexerTx {
    /// Return tx hash
    pub fn tx_hash(&self) -> H256 {
        match self {
            IndexerTx::Ungrouped(tx) => tx.tx_hash.clone(),
            IndexerTx::Grouped(tx) => tx.tx_hash.clone(),
        }
    }
}

/// Ungrouped Tx inner type
#[derive(Serialize)]
pub struct IndexerTxWithCell {
    /// transaction hash
    pub tx_hash: H256,
    /// the number of the transaction committed in the block
    pub block_number: BlockNumber,
    /// the position index of the transaction committed in the block
    pub tx_index: Uint32,
    /// the position index of the cell in the transaction inputs or outputs
    pub io_index: Uint32,
    /// io type
    pub io_type: IndexerCellType,
}

/// Grouped Tx inner type
#[derive(Serialize)]
pub struct IndexerTxWithCells {
    /// transaction hash
    pub tx_hash: H256,
    /// the number of the transaction committed in the block
    pub block_number: BlockNumber,
    /// the position index of the transaction committed in the block
    pub tx_index: Uint32,
    /// Array [[io_type, io_index]]
    pub cells: Vec<(IndexerCellType, Uint32)>,
}

/// Cell type
#[derive(Serialize, Clone)]
#[serde(rename_all = "snake_case")]
pub enum IndexerCellType {
    /// Input
    Input,
    /// Output
    Output,
}
