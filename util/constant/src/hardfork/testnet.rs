/// The Chain Specification name.
pub const CHAIN_SPEC_NAME: &str = "ckb_testnet";

/// hardcode rfc0028 epoch
pub const RFC0028_START_EPOCH: u64 = 3113;
/// First epoch number for CKB v2021, at about 2021/10/24 3:15 UTC.
// pub const CKB2021_START_EPOCH: u64 = 3113;
pub const CKB2021_START_EPOCH: u64 = 0;

// TODO(light-client) update the block number.
/// First epoch which saves the MMR root hash into its header.
pub const RFCTMP1_START_EPOCH: u64 = u64::MAX;
