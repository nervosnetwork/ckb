/// The Chain Specification name.
pub const CHAIN_SPEC_NAME: &str = "ckb";

/// hardcode rfc0028/rfc0032/rfc0033/rfc0034 epoch
pub const RFC0028_RFC0032_RFC0033_RFC0034_START_EPOCH: u64 = 5414;
/// First epoch number for CKB v2021, at about 2022/05/10 1:00 UTC
// pub const CKB2021_START_EPOCH: u64 = 5414;
pub const CKB2021_START_EPOCH: u64 = 0;

/// 2025-07-01 06:32:53 utc
/// |            hash                                                    |  number   |    timestamp    | epoch |
/// | 0xf959f70e487bc3073374d148ef0df713e6060542b84d89a3318bf18edbacdf94 | 15,414,776  |  1739773973982  |  11,489 (1800/1800)|
/// 1739773973982 + 804 * (4 * 60 * 60 * 1000) = 1751351573982  2024-10-25 05:43 utc
pub const CKB2023_START_EPOCH: u64 = 12_293;
