use crate::error::RPCError;
use async_trait::async_trait;
use ckb_dao_utils::extract_dao_data;
use ckb_db_schema::COLUMN_CELL;
use ckb_jsonrpc_types::{
    CellsInfo, Disk, DiskUsage, Global, MiningInfo, Network, NetworkInfo, Overview, PeerInfo,
    SysInfo, TerminalPoolInfo,
};
use ckb_logger::error;
use ckb_network::NetworkController;
use ckb_shared::shared::Shared;
use ckb_store::ChainStore;
use ckb_types::utilities::compact_to_difficulty;
use ckb_util::Mutex;
use jsonrpc_core::Result;
use jsonrpc_utils::rpc;
use lru::LruCache;
use std::sync::Arc;
use std::time::{Duration, Instant};
use sysinfo::{Disks as SysDisks, Networks as SysNetworks, System};

/// Cache TTL constants for different data types
pub mod ttl {
    use std::time::Duration;

    /// System information TTL: 5 seconds - system metrics change relatively frequently
    pub const SYSTEM_INFO: Duration = Duration::from_secs(5);

    /// Mining information TTL: 10 seconds - network difficulty and hash rate change moderately
    pub const MINING_INFO: Duration = Duration::from_secs(10);

    /// Transaction pool TTL: 2 seconds - transaction pool is highly dynamic
    pub const TX_POOL_INFO: Duration = Duration::from_secs(2);

    /// Cells information TTL: 30 seconds - blockchain cell statistics change slowly
    pub const CELLS_INFO: Duration = Duration::from_secs(30);

    /// Network latency TTL: 10 seconds - peer connections and latencies change moderately
    pub const NETWORK_INFO: Duration = Duration::from_secs(10);
}

bitflags::bitflags! {
    /// The bit flags used to determine what to refresh specifically on the Overview type
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct RefreshKind: u32 {
        /// Refresh nothing, use cached data when available
        const NOTHING                      = 0b00000000;
        /// Force refresh system information (CPU, memory, disk, network)
        const SYSTEM_INFO                  = 0b00000001;
        /// Force refresh mining information (difficulty, hash rate)
        const MINING_INFO                  = 0b00000010;
        /// Force refresh transaction pool information
        const TX_POOL_INFO                 = 0b00000100;
        /// Force refresh cells information
        const CELLS_INFO                   = 0b00001000;
        /// Force refresh network peer latency information
        const NETWORK_INFO                 = 0b00010000;
        /// Refresh all cached data
        const EVERYTHING                   = 0b00011111;
    }
}

/// Cache statistics for monitoring
#[derive(Debug, Clone)]
pub struct CacheStats {
    pub sys_info_cached: usize,
    pub mining_info_cached: usize,
    pub tx_pool_info_cached: usize,
    pub cells_info_cached: usize,
    pub network_info_cached: usize,
}

/// Cache entry with timestamp for TTL
#[derive(Clone, Debug)]
struct CacheEntry<T> {
    data: T,
    timestamp: Instant,
}

impl<T> CacheEntry<T> {
    fn new(data: T) -> Self {
        Self {
            data,
            timestamp: Instant::now(),
        }
    }

    fn is_expired(&self, ttl: Duration) -> bool {
        self.timestamp.elapsed() > ttl
    }
}

/// Terminal cache for storing expensive computations
#[derive(Clone)]
pub struct TerminalCache {
    /// System information cache (TTL: 5 seconds)
    sys_info: Arc<Mutex<LruCache<(), CacheEntry<SysInfo>>>>,
    /// Mining information cache (TTL: 10 seconds)
    mining_info: Arc<Mutex<LruCache<(), CacheEntry<MiningInfo>>>>,
    /// Transaction pool information cache (TTL: 2 seconds)
    tx_pool_info: Arc<Mutex<LruCache<(), CacheEntry<TerminalPoolInfo>>>>,
    /// Cells information cache (TTL: 30 seconds)
    cells_info: Arc<Mutex<LruCache<(), CacheEntry<CellsInfo>>>>,
    /// Network information cache (TTL: 10 seconds)
    network_info: Arc<Mutex<LruCache<(), CacheEntry<NetworkInfo>>>>,
}

impl TerminalCache {
    /// Create a new terminal cache with default TTL settings
    pub fn new() -> Self {
        Self {
            sys_info: Arc::new(Mutex::new(LruCache::new(1))),
            mining_info: Arc::new(Mutex::new(LruCache::new(1))),
            tx_pool_info: Arc::new(Mutex::new(LruCache::new(1))),
            cells_info: Arc::new(Mutex::new(LruCache::new(1))),
            network_info: Arc::new(Mutex::new(LruCache::new(1))),
        }
    }

    /// Get cached system info if not expired (TTL: 5 seconds)
    pub fn get_sys_info(&self) -> Option<SysInfo> {
        let mut cache = self.sys_info.lock();
        if let Some(entry) = cache.get(&())
            && !entry.is_expired(ttl::SYSTEM_INFO)
        {
            return Some(entry.data.clone());
        }
        None
    }

    /// Cache system info
    pub fn set_sys_info(&self, info: SysInfo) {
        let mut cache = self.sys_info.lock();
        cache.put((), CacheEntry::new(info));
    }

    /// Get cached mining info if not expired (TTL: 10 seconds)
    pub fn get_mining_info(&self) -> Option<MiningInfo> {
        let mut cache = self.mining_info.lock();
        if let Some(entry) = cache.get(&())
            && !entry.is_expired(ttl::MINING_INFO)
        {
            return Some(entry.data.clone());
        }

        None
    }

    /// Cache mining info
    pub fn set_mining_info(&self, info: MiningInfo) {
        let mut cache = self.mining_info.lock();
        cache.put((), CacheEntry::new(info));
    }

    /// Get cached transaction pool info if not expired (TTL: 2 seconds)
    pub fn get_tx_pool_info(&self) -> Option<TerminalPoolInfo> {
        let mut cache = self.tx_pool_info.lock();
        if let Some(entry) = cache.get(&())
            && !entry.is_expired(ttl::TX_POOL_INFO)
        {
            return Some(entry.data.clone());
        }
        None
    }

    /// Cache transaction pool info
    pub fn set_tx_pool_info(&self, info: TerminalPoolInfo) {
        let mut cache = self.tx_pool_info.lock();
        cache.put((), CacheEntry::new(info));
    }

    /// Get cached cells info if not expired (TTL: 30 seconds)
    pub fn get_cells_info(&self) -> Option<CellsInfo> {
        let mut cache = self.cells_info.lock();
        if let Some(entry) = cache.get(&())
            && !entry.is_expired(ttl::CELLS_INFO)
        {
            return Some(entry.data.clone());
        }

        None
    }

    /// Cache cells info
    pub fn set_cells_info(&self, info: CellsInfo) {
        let mut cache = self.cells_info.lock();
        cache.put((), CacheEntry::new(info));
    }

    /// Get cached network info if not expired (TTL: 10 seconds)
    pub fn get_network_info(&self) -> Option<NetworkInfo> {
        let mut cache = self.network_info.lock();
        if let Some(entry) = cache.get(&())
            && !entry.is_expired(ttl::NETWORK_INFO)
        {
            return Some(entry.data.clone());
        }

        None
    }

    /// Cache network info
    pub fn set_network_info(&self, info: NetworkInfo) {
        let mut cache = self.network_info.lock();
        cache.put((), CacheEntry::new(info));
    }

    /// Clear specific cache entry types based on refresh flags
    pub fn clear_specific(&self, refresh: RefreshKind) {
        if refresh.contains(RefreshKind::SYSTEM_INFO) {
            self.sys_info.lock().clear();
        }
        if refresh.contains(RefreshKind::MINING_INFO) {
            self.mining_info.lock().clear();
        }
        if refresh.contains(RefreshKind::TX_POOL_INFO) {
            self.tx_pool_info.lock().clear();
        }
        if refresh.contains(RefreshKind::CELLS_INFO) {
            self.cells_info.lock().clear();
        }
        if refresh.contains(RefreshKind::NETWORK_INFO) {
            self.network_info.lock().clear();
        }
    }

    /// Get cache statistics for monitoring
    pub fn get_stats(&self) -> CacheStats {
        CacheStats {
            sys_info_cached: self.sys_info.lock().len(),
            mining_info_cached: self.mining_info.lock().len(),
            tx_pool_info_cached: self.tx_pool_info.lock().len(),
            cells_info_cached: self.cells_info.lock().len(),
            network_info_cached: self.network_info.lock().len(),
        }
    }

    /// Clear all cached data
    pub fn clear_all(&self) {
        self.sys_info.lock().clear();
        self.mining_info.lock().clear();
        self.tx_pool_info.lock().clear();
        self.cells_info.lock().clear();
        self.network_info.lock().clear();
    }
}

impl Default for TerminalCache {
    fn default() -> Self {
        Self::new()
    }
}

/// RPC Terminal Module, specifically designed for TUI (Terminal User Interface) applications.
///
/// This module provides optimized endpoints for terminal-based monitoring tools and dashboards,
/// with intelligent caching to minimize performance impact while providing real-time insights.
///
/// # Intended Use Cases
///
/// - **TUI Monitoring Dashboards**: Real-time node status displays in terminal environments
/// - **System Administration**: Command-line tools for node health monitoring
/// - **Resource Monitoring**: Tracking system resource usage over time
/// - **Network Diagnostics**: Monitoring peer connectivity and performance
///
/// # Performance Considerations
///
/// The module uses a multi-tiered caching strategy with TTLs optimized for different data
/// change frequencies. For frequent monitoring calls, use cached data (refresh: null) to
/// minimize system load. Force refresh only when real-time accuracy is critical.
///
/// # Refresh Flags Guide
///
/// Use RefreshKind bit flags strategically:
/// - **Monitoring Mode**: Use `null` or `0` for cached data (recommended)
/// - **Diagnostics Mode**: Use specific flags to refresh relevant data only
/// - **Full Sync**: Use `EVERYTHING` (31) for complete data refresh
#[rpc(openrpc)]
#[async_trait]
pub trait TerminalRpc {
    /// Returns a comprehensive overview of CKB node status for TUI applications.
    ///
    /// This method aggregates system metrics, mining information, transaction pool status,
    /// cells statistics, and network peer information into a single response, optimized
    /// for terminal-based monitoring interfaces.
    ///
    /// ## Params
    ///
    /// * `refresh` - Optional bit flags to force refresh specific cached data types.
    ///   Use `RefreshKind` bit flags to control which data to refresh:
    ///   - `0x1` (SYSTEM_INFO): Force refresh system information (CPU, memory, disk, network)
    ///   - `0x2` (MINING_INFO): Force refresh mining information (difficulty, hash rate)
    ///   - `0x4` (TX_POOL_INFO): Force refresh transaction pool information
    ///   - `0x8` (CELLS_INFO): Force refresh cells information
    ///   - `0x10` (NETWORK_INFO): Force refresh network peer latency information
    ///   - `0x1F` (EVERYTHING): Force refresh all cached data
    ///   - `null` or `0`: Use cached data when available (recommended for frequent calls)
    ///
    /// ## Returns
    ///
    /// Returns an `Overview` structure containing:
    /// - System information (CPU, memory, disk, network metrics)
    /// - Mining information (network difficulty and hash rate)
    /// - Transaction pool statistics
    /// - Blockchain cells information
    /// - Network peer connectivity and latency data
    /// - CKB node version
    ///
    /// ## Cache Behavior
    ///
    /// Data is cached with different TTL values to balance freshness with performance:
    /// - System info: 5 seconds (changes frequently)
    /// - Mining info: 10 seconds (moderate change frequency)
    /// - Transaction pool: 2 seconds (highly dynamic)
    /// - Cells info: 30 seconds (relatively static)
    /// - Network info: 10 seconds (moderate change frequency)
    ///
    /// ## Examples
    ///
    /// Get overview using cached data:
    ///
    /// Request
    ///
    /// ```json
    /// {
    ///   "jsonrpc": "2.0",
    ///   "method": "get_overview",
    ///   "params": [null],
    ///   "id": 1
    /// }
    /// ```
    ///
    /// Response
    ///
    /// ```json
    /// {
    ///   "jsonrpc": "2.0",
    ///   "result": {
    ///     "sys": {
    ///       "cpu_usage": 25.5,
    ///       "memory": 134217728,
    ///       "virtual_memory": 268435456,
    ///       "disk_usage": {
    ///         "read_bytes": 1048576,
    ///         "total_read_bytes": 1073741824,
    ///         "written_bytes": 524288,
    ///         "total_written_bytes": 536870912
    ///       },
    ///       "global": {
    ///         "total_memory": 8589934592,
    ///         "used_memory": 4294967296,
    ///         "global_cpu_usage": 150.0,
    ///         "disks": [
    ///           {
    ///             "total_space": 1000000000000,
    ///             "available_space": 500000000000,
    ///             "is_removable": false
    ///           }
    ///         ],
    ///         "networks": [
    ///           {
    ///             "interface_name": "eth0",
    ///             "received": 1048576,
    ///             "total_received": 1073741824,
    ///             "transmitted": 524288,
    ///             "total_transmitted": 536870912
    ///           }
    ///         ]
    ///       }
    ///     },
    ///     "mining": {
    ///       "difficulty": "0x1e083126",
    ///       "hash_rate": "0x174876e800"
    ///     },
    ///     "pool": {
    ///       "pending": "0x64",
    ///       "proposed": "0x32",
    ///       "orphan": "0x5",
    ///       "committing": "0x1f",
    ///       "total_recent_reject_num": "0x3",
    ///       "total_tx_size": "0x100000",
    ///       "total_tx_cycles": "0x2dc6c",
    ///       "last_txs_updated_at": "0x187b3d137a1",
    ///       "max_tx_pool_size": "0x20000000"
    ///     },
    ///     "cells": {
    ///       "total_occupied_capacities": "0x15f1e59b76c000",
    ///       "estimate_live_cells_num": "0x989680"
    ///     },
    ///    "network": {
    ///    "connected_peers": "0x33",
    ///    "outbound_peers": "0x1f",
    ///    "inbound_peers": "0x14",
    ///    "peers": [
    ///        {
    ///        "peer_id": 0,
    ///        "is_outbound": true,
    ///        "latency_ms": "0x98",
    ///         "address": "/ip4/18.185.102.19/tcp/8115/p2p/QmXwUgF48ULy6hkgfqrEwEfuHW7WyWyWauueRDAYQHNDfN"
    ///        },
    ///        {
    ///        "peer_id": 1,
    ///        "is_outbound": false,
    ///        "latency_ms": "0xa5",
    ///         "address": "/ip4/18.185.102.19/tcp/8115/p2p/QmXwUgF48ULy6hkgfqrEwEfuHW7WyWyWauueRDAYQHNDfN"
    ///        },
    ///        {
    ///        "peer_id": 2,
    ///        "is_outbound": true,
    ///        "latency_ms": "0x0",
    ///         "address": "/ip4/18.185.102.19/tcp/8115/p2p/QmXwUgF48ULy6hkgfqrEwEfuHW7WyWyWauueRDAYQHNDfN"
    ///        },
    ///        {
    ///        "peer_id": 3,
    ///        "is_outbound": true,
    ///        "latency_ms": "0x8c",
    ///         "address": "/ip4/18.185.102.19/tcp/8115/p2p/QmXwUgF48ULy6hkgfqrEwEfuHW7WyWyWauueRDAYQHNDfN"
    ///        }
    ///     ]
    ///    },
    ///     "version": "0.100.0 (abc123def 2023-12-01)"
    ///   },
    ///   "id": 1
    ///  }
    /// ```
    ///
    #[rpc(name = "get_overview")]
    fn get_overview(&self, refresh: Option<u32>) -> Result<Overview>;
}

#[derive(Clone)]
pub(crate) struct TerminalRpcImpl {
    pub shared: Shared,
    pub network_controller: NetworkController,
    pub cache: TerminalCache,
}

#[async_trait]
impl TerminalRpc for TerminalRpcImpl {
    fn get_overview(&self, refresh: Option<u32>) -> Result<Overview> {
        let refresh = refresh
            .and_then(RefreshKind::from_bits)
            .unwrap_or(RefreshKind::NOTHING);

        // If refresh everything, clear cache first
        if refresh.contains(RefreshKind::EVERYTHING) {
            self.cache.clear_all();
        }

        let sys = self.get_sys_info(refresh)?;
        let mining = self.get_mining_info(refresh)?;
        let pool = self.get_tx_pool_info(refresh)?;
        let cells = self.get_cells_info(refresh)?;
        let network = self.get_network_info(refresh)?;

        Ok(Overview {
            sys,
            cells,
            mining,
            pool,
            network,
            version: self.network_controller.version().to_owned(),
        })
    }
}

impl TerminalRpcImpl {
    fn get_mining_info(&self, refresh: RefreshKind) -> Result<MiningInfo> {
        // Check cache first unless force refresh
        if !refresh.contains(RefreshKind::MINING_INFO)
            && let Some(cached) = self.cache.get_mining_info()
        {
            return Ok(cached);
        }

        // Fetch fresh data
        let current_epoch_ext =
            self.shared
                .snapshot()
                .get_current_epoch_ext()
                .ok_or_else(|| {
                    RPCError::custom(
                        RPCError::CKBInternalError,
                        "failed to get current epoch_ext",
                    )
                })?;
        let difficulty = compact_to_difficulty(current_epoch_ext.compact_target());
        let mining_info = MiningInfo {
            difficulty,
            // We use previous_epoch_hash_rate to approximate the full network hash power,
            // simplifying the calculation process.
            hash_rate: current_epoch_ext.previous_epoch_hash_rate().to_owned(),
        };

        // Cache the result
        self.cache.set_mining_info(mining_info.clone());
        Ok(mining_info)
    }

    fn get_sys_info(&self, refresh: RefreshKind) -> Result<SysInfo> {
        // Check cache first unless force refresh
        if !refresh.contains(RefreshKind::SYSTEM_INFO)
            && let Some(cached) = self.cache.get_sys_info()
        {
            return Ok(cached);
        }

        // Fetch fresh system data
        let mut sys = System::new_all();
        sys.refresh_all();

        let total_memory = sys.total_memory();
        let used_memory = sys.used_memory();
        let global_cpu_usage = sys.global_cpu_usage();
        let sys_disks = SysDisks::new_with_refreshed_list();
        let disks = sys_disks
            .iter()
            .map(|disk| Disk {
                total_space: disk.total_space(),
                available_space: disk.available_space(),
                is_removable: disk.is_removable(),
            })
            .collect();
        let sys_networks = SysNetworks::new_with_refreshed_list();
        let networks = sys_networks
            .iter()
            .map(|(name, data)| Network {
                interface_name: name.clone(),
                received: data.received(),
                total_received: data.total_received(),
                transmitted: data.transmitted(),
                total_transmitted: data.total_transmitted(),
            })
            .collect();

        let global = Global {
            total_memory,
            used_memory,
            global_cpu_usage,
            disks,
            networks,
        };

        let process = sys
            .process(
                sysinfo::get_current_pid()
                    .map_err(|e| RPCError::custom(RPCError::CKBInternalError, e))?,
            )
            .ok_or_else(|| {
                RPCError::custom(RPCError::CKBInternalError, "failed to get current process")
            })?;

        let sys_disk_usage = process.disk_usage();
        let sys_info = SysInfo {
            global,
            cpu_usage: process.cpu_usage(),
            memory: process.memory(),
            disk_usage: DiskUsage {
                total_written_bytes: sys_disk_usage.total_written_bytes,
                written_bytes: sys_disk_usage.written_bytes,
                total_read_bytes: sys_disk_usage.total_read_bytes,
                read_bytes: sys_disk_usage.read_bytes,
            },
            virtual_memory: process.virtual_memory(),
        };

        // Cache the result
        self.cache.set_sys_info(sys_info.clone());
        Ok(sys_info)
    }

    fn get_tx_pool_info(&self, refresh: RefreshKind) -> Result<TerminalPoolInfo> {
        // Check cache first unless force refresh
        if !refresh.contains(RefreshKind::TX_POOL_INFO)
            && let Some(cached) = self.cache.get_tx_pool_info()
        {
            return Ok(cached);
        }

        // Fetch fresh transaction pool data
        let tx_pool = self.shared.tx_pool_controller();
        let get_tx_pool_info = tx_pool.get_tx_pool_info();
        if let Err(e) = get_tx_pool_info {
            error!("Send get_tx_pool_info request error {}", e);
            return Err(RPCError::ckb_internal_error(e));
        };

        let info = get_tx_pool_info.unwrap();

        let block_template = self
            .shared
            .get_block_template(None, None, None)
            .map_err(|err| {
                error!("Send get_block_template request error {}", err);
                RPCError::ckb_internal_error(err)
            })?
            .map_err(|err| {
                error!("Get_block_template result error {}", err);
                RPCError::from_any_error(err)
            })?;

        let total_recent_reject_num = tx_pool.get_total_recent_reject_num().map_err(|err| {
            error!("Get_total_recent_reject_num result error {}", err);
            RPCError::from_any_error(err)
        })?;

        let tx_pool_info = TerminalPoolInfo {
            pending: (info.pending_size as u64).into(),
            proposed: (info.proposed_size as u64).into(),
            orphan: (info.orphan_size as u64).into(),
            committing: (block_template.transactions.len() as u64).into(),
            total_recent_reject_num: total_recent_reject_num.unwrap_or(0).into(),
            total_tx_size: (info.total_tx_size as u64).into(),
            total_tx_cycles: info.total_tx_cycles.into(),
            last_txs_updated_at: info.last_txs_updated_at.into(),
            max_tx_pool_size: info.max_tx_pool_size.into(),
        };

        // Cache the result
        self.cache.set_tx_pool_info(tx_pool_info.clone());
        Ok(tx_pool_info)
    }

    fn get_cells_info(&self, refresh: RefreshKind) -> Result<CellsInfo> {
        // Check cache first unless force refresh
        if !refresh.contains(RefreshKind::CELLS_INFO)
            && let Some(cached) = self.cache.get_cells_info()
        {
            return Ok(cached);
        }

        // Fetch fresh cells data
        let snapshot = self.shared.cloned_snapshot();
        let tip_header = snapshot.tip_header();
        let (_ar, _c, _s, u) = extract_dao_data(tip_header.dao());
        let estimate_live_cells_num = self
            .shared
            .store()
            .estimate_num_keys_cf(COLUMN_CELL)
            .map_err(|err| {
                error!("estimate_num_keys_cf error {}", err);
                RPCError::ckb_internal_error(err)
            })?;

        let cells_info = CellsInfo {
            total_occupied_capacities: u.into(),
            estimate_live_cells_num: estimate_live_cells_num.unwrap_or(0).into(),
        };

        // Cache the result
        self.cache.set_cells_info(cells_info.clone());
        Ok(cells_info)
    }

    fn get_network_info(&self, refresh: RefreshKind) -> Result<NetworkInfo> {
        // Check cache first unless force refresh
        if !refresh.contains(RefreshKind::NETWORK_INFO)
            && let Some(cached) = self.cache.get_network_info()
        {
            return Ok(cached);
        }

        // Fetch fresh network data
        let peers = self.network_controller.connected_peers();
        let total_peers = peers.len();
        let mut outbound_peers = 0;
        let mut inbound_peers = 0;
        let mut peer_infos = Vec::new();

        for (peer_index, peer) in peers {
            // Count inbound vs outbound connections
            if peer.is_outbound() {
                outbound_peers += 1;
            } else {
                inbound_peers += 1;
            }

            // Extract peer ID and RTT information
            let peer_id = peer_index.value();
            let is_outbound = peer.is_outbound();
            let latency_ms = if let Some(rtt) = peer.ping_rtt {
                rtt.as_millis() as u64
            } else {
                0
            };

            peer_infos.push(PeerInfo {
                peer_id,
                is_outbound,
                latency_ms: latency_ms.into(),
                address: peer.connected_addr.to_string(),
            });
        }

        let network_info = NetworkInfo {
            connected_peers: (total_peers as u64).into(),
            outbound_peers: (outbound_peers as u64).into(),
            inbound_peers: (inbound_peers as u64).into(),
            peers: peer_infos,
        };

        // Cache the result
        self.cache.set_network_info(network_info.clone());
        Ok(network_info)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_cache_entry_expiration() {
        let entry = CacheEntry::new("test_data");
        assert!(!entry.is_expired(Duration::from_secs(1)));

        // Even with 0ms duration, there might be a tiny delay due to execution time
        // so we just test that it doesn't panic and returns a boolean
        let _ = entry.is_expired(Duration::from_millis(0));
    }

    #[test]
    fn test_refresh_kind_flags() {
        let nothing = RefreshKind::NOTHING;
        assert!(!nothing.contains(RefreshKind::SYSTEM_INFO));
        assert!(!nothing.contains(RefreshKind::MINING_INFO));

        let system = RefreshKind::SYSTEM_INFO;
        assert!(system.contains(RefreshKind::SYSTEM_INFO));
        assert!(!system.contains(RefreshKind::MINING_INFO));

        let all = RefreshKind::EVERYTHING;
        assert!(all.contains(RefreshKind::SYSTEM_INFO));
        assert!(all.contains(RefreshKind::MINING_INFO));
        assert!(all.contains(RefreshKind::TX_POOL_INFO));
        assert!(all.contains(RefreshKind::CELLS_INFO));
    }

    #[test]
    fn test_terminal_cache_basic_operations() {
        let cache = TerminalCache::new();

        // Test that cache is initially empty
        assert!(cache.get_sys_info().is_none());
        assert!(cache.get_mining_info().is_none());

        // Test setting and getting values
        let sys_info = SysInfo {
            global: Global {
                total_memory: 1000,
                used_memory: 500,
                global_cpu_usage: 50.0,
                disks: vec![],
                networks: vec![],
            },
            cpu_usage: 0.0,
            memory: 0,
            disk_usage: DiskUsage {
                total_written_bytes: 0,
                written_bytes: 0,
                total_read_bytes: 0,
                read_bytes: 0,
            },
            virtual_memory: 0,
        };

        cache.set_sys_info(sys_info);
        assert!(cache.get_sys_info().is_some());

        // Test clear all
        cache.clear_all();
        assert!(cache.get_sys_info().is_none());
    }

    #[test]
    fn test_network_info_basic_operations() {
        let cache = TerminalCache::new();

        // Test that network cache is initially empty
        assert!(cache.get_network_info().is_none());

        // Test setting and getting network info
        let network_info = NetworkInfo {
            connected_peers: 5u64.into(),
            outbound_peers: 3u64.into(),
            inbound_peers: 2u64.into(),
            peers: vec![
                PeerInfo {
                    peer_id: 0,
                    is_outbound: true,
                    latency_ms: 150u64.into(),
                    address: "/ip4/192.168.1.100/tcp/8114".to_string(),
                },
                PeerInfo {
                    peer_id: 1,
                    is_outbound: true,
                    latency_ms: 50u64.into(),
                    address: "/ip4/192.168.1.101/tcp/8114".to_string(),
                },
                PeerInfo {
                    peer_id: 2,
                    is_outbound: false,
                    latency_ms: 300u64.into(),
                    address: "/ip4/192.168.1.102/tcp/8114".to_string(),
                },
                PeerInfo {
                    peer_id: 3,
                    is_outbound: false,
                    latency_ms: 100u64.into(),
                    address: "/ip4/192.168.1.103/tcp/8114".to_string(),
                },
                PeerInfo {
                    peer_id: 4,
                    is_outbound: true,
                    latency_ms: 0u64.into(),
                    address: "/ip4/192.168.1.104/tcp/8114".to_string(),
                },
            ],
        };

        cache.set_network_info(network_info);
        let cached = cache
            .get_network_info()
            .expect("Should have cached network info");

        assert_eq!(cached.connected_peers, 5u64.into());
        assert_eq!(cached.outbound_peers, 3u64.into());
        assert_eq!(cached.inbound_peers, 2u64.into());
        assert_eq!(cached.peers.len(), 5);
        assert_eq!(cached.peers[0].peer_id, 0);
        assert!(cached.peers[0].is_outbound);
        assert_eq!(cached.peers[0].latency_ms, 150u64.into());
    }

    #[test]
    fn test_refresh_kind_network_flag() {
        let nothing = RefreshKind::NOTHING;
        assert!(!nothing.contains(RefreshKind::NETWORK_INFO));

        let network = RefreshKind::NETWORK_INFO;
        assert!(network.contains(RefreshKind::NETWORK_INFO));
        assert!(!network.contains(RefreshKind::SYSTEM_INFO));

        let all = RefreshKind::EVERYTHING;
        assert!(all.contains(RefreshKind::NETWORK_INFO));
        assert!(all.contains(RefreshKind::SYSTEM_INFO));
        assert!(all.contains(RefreshKind::MINING_INFO));
        assert!(all.contains(RefreshKind::TX_POOL_INFO));
        assert!(all.contains(RefreshKind::CELLS_INFO));
    }

    #[test]
    fn test_cache_stats_includes_network() {
        let cache = TerminalCache::new();
        let stats = cache.get_stats();

        // All cache stats should be 0 initially
        assert_eq!(stats.sys_info_cached, 0);
        assert_eq!(stats.mining_info_cached, 0);
        assert_eq!(stats.tx_pool_info_cached, 0);
        assert_eq!(stats.cells_info_cached, 0);
        assert_eq!(stats.network_info_cached, 0);

        // Add network info and check stats
        let network_info = NetworkInfo::default();
        cache.set_network_info(network_info);

        let stats = cache.get_stats();
        assert_eq!(stats.network_info_cached, 1);
    }
}
