//! Legacy CKB AppConfig and Miner AppConfig

use ckb_jsonrpc_types::ChainEdition;
use serde::{Deserialize, Serialize};

pub(crate) mod v2019;

/// The partial CKB AppConfig or Miner AppConfig
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub(crate) struct PartialAppConfig {
    pub(crate) edition: Option<ChainEdition>,
}
