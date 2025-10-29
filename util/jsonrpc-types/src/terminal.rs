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
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
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
