//! Legacy CKB Chain Specification (Edition 2019)

use ckb_jsonrpc_types as rpc;
use ckb_pow::Pow;
use ckb_types::packed;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct ChainSpec {
    name: String,
    pub(crate) genesis: crate::Genesis,
    #[serde(default)]
    params: crate::Params,
    pow: Pow,
    #[serde(skip)]
    pub(crate) hash: packed::Byte32,
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
            genesis,
            params,
            pow,
            hash,
            legacy: true,
        }
    }
}
