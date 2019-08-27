mod alert;
mod block_template;
mod blockchain;
mod bytes;
mod cell;
mod chain_info;
mod experiment;
mod fixed_bytes;
mod indexer;
mod net;
mod pool;
mod proposal_short_id;
mod string;
mod sync;

use ckb_types::core;

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct BlockNumber(#[serde(with = "string")] pub core::BlockNumber);

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct Capacity(#[serde(with = "string")] pub core::Capacity);

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct Cycle(#[serde(with = "string")] pub core::Cycle);

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct EpochNumber(#[serde(with = "string")] pub core::EpochNumber);

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct Version(#[serde(with = "string")] pub core::Version);

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct Timestamp(#[serde(with = "string")] pub u64);

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct Unsigned(#[serde(with = "string")] pub u64);

pub use self::alert::{Alert, AlertMessage};
pub use self::block_template::{
    BlockTemplate, CellbaseTemplate, TransactionTemplate, UncleTemplate,
};
pub use self::blockchain::{
    Block, BlockReward, BlockView, CellDep, CellInput, CellOutput, DepType, EpochView, Header,
    HeaderView, OutPoint, Script, ScriptHashType, Status, Transaction, TransactionView,
    TransactionWithStatus, TxStatus, UncleBlock, UncleBlockView, Witness,
};
pub use self::bytes::JsonBytes;
pub use self::cell::{CellOutputWithOutPoint, CellWithStatus};
pub use self::chain_info::ChainInfo;
pub use self::experiment::DryRunResult;
pub use self::fixed_bytes::Byte32;
pub use self::indexer::{CellTransaction, LiveCell, LockHashIndexState, TransactionPoint};
pub use self::net::{BannedAddress, Node, NodeAddress};
pub use self::pool::TxPoolInfo;
pub use self::proposal_short_id::ProposalShortId;
pub use self::sync::PeerState;
pub use jsonrpc_core::types::{error, id, params, request, response, version};
pub use serde_derive::{Deserialize, Serialize};
