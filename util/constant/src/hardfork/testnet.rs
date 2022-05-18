/// The Chain Specification name.
pub const CHAIN_SPEC_NAME: &str = "ckb_testnet";

/// First epoch number for CKB v2021, at about 2021/10/24 3:15 UTC.
pub const CKB2021_START_EPOCH: u64 = 3113;

// TODO(light-client) update the block number.
/// First block which saves the MMR root hash into its header.
pub const MMR_ACTIVATED_BLOCK: u64 = u64::MAX;
