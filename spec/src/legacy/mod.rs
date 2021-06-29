//! Legacy CKB Chain Specification

use ckb_jsonrpc_types::ChainEdition;
use serde::{Deserialize, Serialize};

pub(crate) mod v2019;

/// The partial CKB block chain specification
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub(crate) struct PartialChainSpec {
    pub(crate) edition: Option<ChainEdition>,
}
