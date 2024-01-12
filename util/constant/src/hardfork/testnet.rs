/// The Chain Specification name.
pub const CHAIN_SPEC_NAME: &str = "ckb_testnet";

/// hardcode rfc0028/rfc0032/rfc0033/rfc0034 epoch
pub const RFC0028_RFC0032_RFC0033_RFC0034_START_EPOCH: u64 = 3113;
/// First epoch number for CKB v2021, at about 2021/10/24 3:15 UTC.
// pub const CKB2021_START_EPOCH: u64 = 3113;
pub const CKB2021_START_EPOCH: u64 = 0;

/// hardcode ckb2023 epoch
/// 2024/02/05 7:44 utc
/// |            hash                                                    |  number   |    timestamp    | epoch |
/// | 0x4014244087d989355c1d06311f3f73582607d4c991e03296f703f9221f77aabe | 11937425  |  1704944649417  |  9074 |
/// 1704944649417 + 151 * 4 * 60 * 60 * 1000 = 1707119049417  2024/02/05 7:44 utc
pub const CKB2023_START_EPOCH: u64 = 9225;
