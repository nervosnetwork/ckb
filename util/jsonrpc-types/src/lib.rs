//! Wrappers for JSON serialization.
mod alert;
mod block_template;
mod blockchain;
mod bytes;
mod cell;
mod debug;
mod experiment;
mod fee_rate;
mod fixed_bytes;
mod indexer;
mod info;
mod net;
mod pool;
mod primitive;
mod proposal_short_id;
mod subscription;
mod uints;

#[cfg(test)]
mod tests;

pub use self::alert::{Alert, AlertId, AlertMessage, AlertPriority};
pub use self::block_template::{
    BlockTemplate, CellbaseTemplate, TransactionTemplate, UncleTemplate,
};
pub use self::blockchain::{
    Block, BlockEconomicState, BlockIssuance, BlockResponse, BlockView, CellDep, CellInput,
    CellOutput, Consensus, DepType, EpochView, FeeRateStatics, HardForkFeature, Header, HeaderView,
    MerkleProof, MinerReward, OutPoint, ProposalWindow, Script, ScriptHashType, Status,
    Transaction, TransactionProof, TransactionView, TransactionWithStatusResponse, TxStatus,
    UncleBlock, UncleBlockView,
};
pub use self::bytes::JsonBytes;
pub use self::cell::{CellData, CellInfo, CellWithStatus};
pub use self::debug::{ExtraLoggerConfig, MainLoggerConfig};
pub use self::experiment::{DaoWithdrawingCalculationKind, EstimateCycles};
pub use self::fee_rate::FeeRateDef;
pub use self::fixed_bytes::Byte32;
pub use self::info::{ChainInfo, DeploymentInfo, DeploymentPos, DeploymentState, DeploymentsInfo};
pub use self::net::{
    BannedAddr, LocalNode, LocalNodeProtocol, NodeAddress, PeerSyncState, RemoteNode,
    RemoteNodeProtocol, SyncState,
};
pub use self::pool::{
    OutputsValidator, PoolTransactionEntry, PoolTransactionReject, RawTxPool, TxPoolEntries,
    TxPoolEntry, TxPoolIds, TxPoolInfo,
};
pub use self::proposal_short_id::ProposalShortId;
pub use self::subscription::Topic;
pub use self::uints::{Uint128, Uint32, Uint64};
pub use indexer::{
    IndexerCell, IndexerCellType, IndexerCellsCapacity, IndexerOrder, IndexerPagination,
    IndexerRange, IndexerScriptType, IndexerSearchKey, IndexerSearchKeyFilter, IndexerTip,
    IndexerTx, IndexerTxWithCell, IndexerTxWithCells,
};
pub use primitive::{
    AsEpochNumberWithFraction, BlockNumber, Capacity, Cycle, EpochNumber, EpochNumberWithFraction,
    Timestamp, Version,
};
pub use serde::{Deserialize, Serialize};

use ckb_types::bytes::Bytes;

/// The enum `Either` with variants `Left` and `Right` is a general purpose
/// sum type with two cases.
#[derive(PartialEq, Eq, Hash, Debug, Serialize, Deserialize, Clone)]
#[serde(untagged)]
pub enum Either<L, R> {
    /// A value of type `L`.
    Left(L),
    /// A value of type `R`.
    Right(R),
}

/// This is a wrapper for JSON serialization to select the format between Json and Hex.
///
/// ## Examples
///
/// `ResponseFormat<BlockView>` returns the block in its Json format or molecule serialized
/// Hex format.
#[derive(PartialEq, Eq, Hash, Debug, Serialize, Deserialize, Clone)]
#[serde(transparent)]
pub struct ResponseFormat<V> {
    /// The inner value.
    pub inner: Either<V, JsonBytes>,
}

/// The enum `ResponseFormatInnerType` with variants `Json` and `Hex` is is used to
/// supply a format choice for the format of `ResponseFormatResponse.transaction`
pub enum ResponseFormatInnerType {
    /// Indicate the json format of `ResponseFormatResponse.transaction`
    Json,
    /// Indicate the hex format of `ResponseFormatResponse.transaction`
    Hex,
}

impl<V> ResponseFormat<V> {
    /// Return json format
    pub fn json(json: V) -> Self {
        ResponseFormat {
            inner: Either::Left(json),
        }
    }

    /// Return molecule serialized hex format
    pub fn hex(raw: Bytes) -> Self {
        ResponseFormat {
            inner: Either::Right(JsonBytes::from_bytes(raw)),
        }
    }
}
