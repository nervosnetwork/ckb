use std::time::Duration;

/// The default init download block interval is 24 hours
/// If the time of the local highest block is within this range, exit the ibd state
pub const MAX_TIP_AGE: u64 = 24 * 60 * 60 * 1000;

/// Default max get header response length, if it is greater than this value, the message will be ignored
pub const MAX_HEADERS_LEN: usize = 2_000;

/// The default number of download blocks that can be requested at one time
/* About Download Scheduler */
pub const INIT_BLOCKS_IN_TRANSIT_PER_PEER: usize = 16;
/// Maximum number of download blocks that can be requested at one time
pub const MAX_BLOCKS_IN_TRANSIT_PER_PEER: usize = 128;
/// The point at which the scheduler adjusts the number of tasks, by default one adjustment per 512 blocks.
pub const CHECK_POINT_WINDOW: u64 = (MAX_BLOCKS_IN_TRANSIT_PER_PEER * 4) as u64;

/// Inspect the headers downloading every 2 minutes
pub const HEADERS_DOWNLOAD_INSPECT_WINDOW: u64 = 2 * 60 * 1000;
/// Global Average Speed
//      Expect 300 KiB/second
//          = 1600 headers/second (300*1024/192)
//          = 96000 headers/minute (1600*60)
//          = 11.11 days-in-blockchain/minute-in-reality (96000*10/60/60/24)
//      => Sync 1 year headers in blockchain will be in 32.85 minutes (365/11.11) in reality
pub const HEADERS_DOWNLOAD_HEADERS_PER_SECOND: u64 = 1600;
/// Acceptable Lowest Instantaneous Speed: 75.0 KiB/second (300/4)
pub const HEADERS_DOWNLOAD_TOLERABLE_BIAS_FOR_SINGLE_SAMPLE: u64 = 4;
/// Pow interval
pub const POW_INTERVAL: u64 = 10;

/// Protect at least this many outbound peers from disconnection due to slow
/// behind headers chain.
pub const MAX_OUTBOUND_PEERS_TO_PROTECT_FROM_DISCONNECT: usize = 4;
/// Chain sync timout
pub const CHAIN_SYNC_TIMEOUT: u64 = 12 * 60 * 1000; // 12 minutes
/// Suspend sync time
pub const SUSPEND_SYNC_TIME: u64 = 5 * 60 * 1000; // 5 minutes
/// Eviction response time
pub const EVICTION_HEADERS_RESPONSE_TIME: u64 = 120 * 1000; // 2 minutes

/// The maximum number of entries in a locator
pub const MAX_LOCATOR_SIZE: usize = 101;

/// Block download timeout
pub const BLOCK_DOWNLOAD_TIMEOUT: u64 = 30 * 1000; // 30s

/// Block download window size
// Size of the "block download window": how far ahead of our current height do we fetch?
// Larger windows tolerate larger download speed differences between peers, but increase the
// potential degree of disordering of blocks.
pub const BLOCK_DOWNLOAD_WINDOW: u64 = 1024 * 8; // 1024 * default_outbound_peers

/// Interval between repeated inquiry transactions
pub const RETRY_ASK_TX_TIMEOUT_INCREASE: Duration = Duration::from_secs(30);

/// Default ban time for message
// ban time
// 5 minutes
pub const BAD_MESSAGE_BAN_TIME: Duration = Duration::from_secs(5 * 60);
/// Default ban time for sync useless
// 10 minutes, peer have no common ancestor block
pub const SYNC_USELESS_BAN_TIME: Duration = Duration::from_secs(10 * 60);
