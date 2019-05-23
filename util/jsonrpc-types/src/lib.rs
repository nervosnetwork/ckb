mod alert;
mod block_template;
mod blockchain;
mod bytes;
mod cell;
mod chain_info;
mod experiment;
mod net;
mod pool;
mod proposal_short_id;
mod string;
mod sync;

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct BlockNumber(#[serde(with = "string")] pub ckb_core::BlockNumber);

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct Capacity(#[serde(with = "string")] pub ckb_core::Capacity);

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct Cycle(#[serde(with = "string")] pub ckb_core::Cycle);

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct EpochNumber(#[serde(with = "string")] pub ckb_core::EpochNumber);

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct Version(#[serde(with = "string")] pub ckb_core::Version);

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct Timestamp(#[serde(with = "string")] pub u64);

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct Unsigned(#[serde(with = "string")] pub u64);

pub use self::alert::Alert;
pub use self::block_template::{
    BlockTemplate, CellbaseTemplate, TransactionTemplate, UncleTemplate,
};
pub use self::blockchain::{
    Block, BlockView, CellInput, CellOutPoint, CellOutput, EpochExt, Header, HeaderView, OutPoint,
    Script, Seal, Transaction, TransactionView, TransactionWithStatus, TxStatus, UncleBlock,
    UncleBlockView, Witness,
};
pub use self::bytes::JsonBytes;
pub use self::cell::{CellOutputWithOutPoint, CellWithStatus};
pub use self::chain_info::ChainInfo;
pub use self::experiment::DryRunResult;
pub use self::net::{Node, NodeAddress};
pub use self::pool::TxPoolInfo;
pub use self::proposal_short_id::ProposalShortId;
pub use self::sync::PeerState;
pub use jsonrpc_core::types::{error, id, params, request, response, version};
pub use serde_derive::{Deserialize, Serialize};
