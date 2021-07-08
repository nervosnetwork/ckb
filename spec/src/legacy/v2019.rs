//! Legacy CKB Chain Specification (Edition 2019)

use ckb_jsonrpc_types as rpc;
use ckb_pow::Pow;
use ckb_types::{core, packed};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct ChainSpec {
    name: String,
    pub(crate) genesis: crate::Genesis,
    #[serde(default)]
    params: Params,
    pow: Pow,
    #[serde(skip)]
    pub(crate) hash: packed::Byte32,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
struct Params {
    #[serde(skip_serializing_if = "Option::is_none")]
    initial_primary_epoch_reward: Option<core::Capacity>,
    #[serde(skip_serializing_if = "Option::is_none")]
    secondary_epoch_reward: Option<core::Capacity>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_block_cycles: Option<core::Cycle>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_block_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cellbase_maturity: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    primary_epoch_reward_halving_interval: Option<core::EpochNumber>,
    #[serde(skip_serializing_if = "Option::is_none")]
    epoch_duration_target: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    genesis_epoch_length: Option<core::BlockNumber>,
    #[serde(skip_serializing_if = "Option::is_none")]
    permanent_difficulty_in_dummy: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_block_proposals_limit: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    orphan_rate_target: Option<(u32, u32)>,
}

impl From<Params> for crate::Params {
    fn from(input: Params) -> Self {
        let Params {
            initial_primary_epoch_reward,
            secondary_epoch_reward,
            max_block_cycles,
            max_block_bytes,
            cellbase_maturity,
            primary_epoch_reward_halving_interval,
            epoch_duration_target,
            genesis_epoch_length,
            permanent_difficulty_in_dummy,
            max_block_proposals_limit,
            orphan_rate_target,
        } = input;
        Self {
            initial_primary_epoch_reward,
            secondary_epoch_reward,
            max_block_cycles,
            max_block_bytes,
            cellbase_maturity,
            primary_epoch_reward_halving_interval,
            epoch_duration_target,
            genesis_epoch_length,
            permanent_difficulty_in_dummy,
            max_block_proposals_limit,
            orphan_rate_target,
            hardfork: None,
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
            genesis,
            params: params.into(),
            pow,
            hash,
            legacy: true,
        }
    }
}
