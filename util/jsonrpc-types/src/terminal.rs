use crate::{Capacity, Timestamp, Uint64};
use ckb_types::U256;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Overview data structure aggregating system and mining information
/// for the CKB-TUI Terminal module.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct Overview {
    /// System information, including hardware and OS metrics.
    pub sys: SysInfo,
    /// Mining information, covering hash power, difficulty.
    pub mining: MiningInfo,
    /// Transaction pool information.
    pub pool: TerminalPoolInfo,
    /// Cells information.
    pub cells: CellsInfo,
    /// Network peer latency information.
    pub network: NetworkInfo,
    /// CKB node version.
    ///
    /// Example: "version": "0.34.0 (f37f598 2020-07-17)"
    pub version: String,
}

/// System’s information.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct SysInfo {
    /// global system information
    pub global: Global,
    /// Returns the total ckb CPU usage (in %).
    /// Notice that it might be bigger than 100 if run on a multi-core machine.
    pub cpu_usage: f32,
    /// Returns number of bytes ckb read and written to disk.
    pub disk_usage: DiskUsage,
    /// Returns the memory ckb usage (in bytes).
    pub memory: u64,
    /// Returns the virtual memory usage (in bytes).
    pub virtual_memory: u64,
}

/// Global system information
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct Global {
    /// Returns the RAM size in bytes.
    pub total_memory: u64,
    /// Returns the amount of used RAM in bytes.
    pub used_memory: u64,
    /// Returns “global” CPUs usage (aka the addition of all the CPUs).
    pub global_cpu_usage: f32,
    /// Returns disks information.
    pub disks: Vec<Disk>,
    /// Returns networks information.
    pub networks: Vec<Network>,
}

/// Struct containing a disk information.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct Disk {
    /// Returns the total disk size, in bytes.
    pub total_space: u64,
    /// Returns the available disk size, in bytes.
    pub available_space: u64,
    /// Returns true if the disk is removable.
    pub is_removable: bool,
}

/// Struct containing read and written bytes.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, Default)]
pub struct DiskUsage {
    /// Total number of written bytes.
    pub total_written_bytes: u64,
    /// Number of written bytes since the last refresh.
    pub written_bytes: u64,
    /// Total number of read bytes.
    pub total_read_bytes: u64,
    /// Number of read bytes since the last refresh.
    pub read_bytes: u64,
}

/// Getting volume of received and transmitted data.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct Network {
    /// Returns network interface name
    pub interface_name: String,
    /// Returns the number of received bytes since the last refresh.
    pub received: u64,
    /// Returns the total number of received bytes.
    pub total_received: u64,
    /// Returns the number of transmitted bytes since the last refresh.
    pub transmitted: u64,
    /// Returns the total number of transmitted bytes.
    pub total_transmitted: u64,
}

/// Mining information structure for the CKB-TUI Terminal module.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct MiningInfo {
    /// Current network difficulty, represented as a U256 integer.
    #[schemars(schema_with = "crate::json_schema::u256_json_schema")]
    pub difficulty: U256,
    /// Current network hash rate, represented as a U256 integer (in hashes per second).
    /// This approximates the total computational power of the mining network.
    #[schemars(schema_with = "crate::json_schema::u256_json_schema")]
    pub hash_rate: U256,
}

/// Transaction pool information.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct TerminalPoolInfo {
    /// Count of transactions in the pending state.
    ///
    /// The pending transactions must be proposed in a new block first.
    pub pending: Uint64,
    /// Count of transactions in the proposed state.
    ///
    /// The proposed transactions are ready to be committed in the new block after the block
    /// `tip_hash`.
    pub proposed: Uint64,
    /// Count of orphan transactions.
    ///
    /// An orphan transaction has an input cell from the transaction which is neither in the chain
    /// nor in the transaction pool.
    pub orphan: Uint64,
    /// Count of committing transactions.
    ///
    /// The Committing transactions refer to transactions that have been packaged into the
    /// block_template and are awaiting mining into a block.
    pub committing: Uint64,
    /// Total count of recent reject transactions by pool
    pub total_recent_reject_num: Uint64,
    /// Total size of transactions bytes in the pool of all the different kinds of states (excluding orphan transactions).
    pub total_tx_size: Uint64,
    /// Total consumed VM cycles of all the transactions in the pool (excluding orphan transactions).
    pub total_tx_cycles: Uint64,
    /// Total limit on the size of transactions in the tx-pool
    pub max_tx_pool_size: Uint64,
    /// Last updated time. This is the Unix timestamp in milliseconds.
    pub last_txs_updated_at: Timestamp,
}

/// Individual peer connection information.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct PeerInfo {
    /// Unique identifier for peer
    pub peer_id: usize,
    /// Whether this is an outbound connection
    pub is_outbound: bool,
    /// Round-trip time in milliseconds for this peer (0 if not available)
    pub latency_ms: Uint64,
    /// Peer address
    pub address: String,
}

/// Network peer latency information.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct NetworkInfo {
    /// Total number of connected peers
    pub connected_peers: Uint64,
    /// Number of outbound connections
    pub outbound_peers: Uint64,
    /// Number of inbound connections
    pub inbound_peers: Uint64,
    /// List of individual peer information with their latencies
    pub peers: Vec<PeerInfo>,
}

/// Cells information.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct CellsInfo {
    /// estimate live cells total num
    pub estimate_live_cells_num: Uint64,
    ///  The total occupied capacities currently in the CKB
    pub total_occupied_capacities: Capacity,
}

impl Default for CellsInfo {
    fn default() -> Self {
        Self {
            estimate_live_cells_num: 0u64.into(),
            total_occupied_capacities: 0u64.into(),
        }
    }
}

impl Default for TerminalPoolInfo {
    fn default() -> Self {
        Self {
            pending: 0u64.into(),
            proposed: 0u64.into(),
            orphan: 0u64.into(),
            committing: 0u64.into(),
            total_recent_reject_num: 0u64.into(),
            total_tx_size: 0u64.into(),
            total_tx_cycles: 0u64.into(),
            max_tx_pool_size: 0u64.into(),
            last_txs_updated_at: 0u64.into(),
        }
    }
}

impl Default for MiningInfo {
    fn default() -> Self {
        Self {
            difficulty: U256::zero(),
            hash_rate: U256::zero(),
        }
    }
}

impl Default for SysInfo {
    fn default() -> Self {
        Self {
            global: Global::default(),
            cpu_usage: 0.0,
            memory: 0,
            virtual_memory: 0,
            disk_usage: DiskUsage::default(),
        }
    }
}

impl Default for Global {
    fn default() -> Self {
        Self {
            total_memory: 0,
            used_memory: 0,
            global_cpu_usage: 0.0,
            disks: Vec::new(),
            networks: Vec::new(),
        }
    }
}

impl Default for PeerInfo {
    fn default() -> Self {
        Self {
            peer_id: 0,
            is_outbound: false,
            latency_ms: 0u64.into(),
            address: String::new(),
        }
    }
}

impl Default for NetworkInfo {
    fn default() -> Self {
        Self {
            connected_peers: 0u64.into(),
            outbound_peers: 0u64.into(),
            inbound_peers: 0u64.into(),
            peers: Vec::new(),
        }
    }
}
