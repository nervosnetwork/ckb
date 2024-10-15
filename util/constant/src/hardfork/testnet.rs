/// The Chain Specification name.
pub const CHAIN_SPEC_NAME: &str = "ckb_testnet";

/// hardcode rfc0028/rfc0032/rfc0033/rfc0034 epoch
pub const RFC0028_RFC0032_RFC0033_RFC0034_START_EPOCH: u64 = 3113;
/// First epoch number for CKB v2021, at about 2021/10/24 3:15 UTC.
// pub const CKB2021_START_EPOCH: u64 = 3113;
pub const CKB2021_START_EPOCH: u64 = 0;

/// 2024-10-25 05:43 utc
/// |            hash                                                    |  number   |    timestamp    | epoch |
/// | 0xa229cd72240f6ef238681e21d1e6884b825afce07e2394308411facfb3cd64c2 | 14,691,304  |  1727243008225  |  9510 (1800/1800)|
/// 1727243008225 + 180 * (4 * 60 * 60 * 1000) = 1729835008225  2024-10-25 05:43 utc
pub const CKB2023_START_EPOCH: u64 = 9690;
