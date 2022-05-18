/// The Chain Specification name.
pub const CHAIN_SPEC_NAME: &str = "ckb";

/// hardcode rfc0028 epoch
pub const RFC0028_START_EPOCH: u64 = 5414;
/// First epoch number for CKB v2021, at about 2022/05/10 1:00 UTC
// pub const CKB2021_START_EPOCH: u64 = 5414;
pub const CKB2021_START_EPOCH: u64 = 0;

// TODO(light-client) update the block number.
/// First block which saves the MMR root hash into its header.
pub const MMR_ACTIVATED_BLOCK: u64 = u64::MAX;
