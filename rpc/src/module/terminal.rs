use crate::error::RPCError;
use async_trait::async_trait;
use ckb_jsonrpc_types::{Disk, DiskUsage, Global, MiningInfo, Network, Overview, SysInfo};
use ckb_shared::shared::Shared;
use ckb_store::ChainStore;
use ckb_types::utilities::compact_to_target;
use jsonrpc_core::Result;
use jsonrpc_utils::rpc;
use sysinfo::{Disks as SysDisks, Networks as SysNetworks, System};

bitflags::bitflags! {
    /// The bit flags used to determine what to refresh specifically on the Overview type
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct RefreshKind: u32 {
        /// None of verifier will be disabled
        const NOTHING                      = 0b00000000;
        // const EVERYTHING                   = 0b00000000;
    }
}

/// RPC Terminal Module, specifically designed for TUI (Terminal User Interface) applications.
#[rpc(openrpc)]
#[async_trait]
pub trait TerminalRpc {
    #[rpc(name = "get_overview")]
    fn get_overview(&self, refresh: Option<u32>) -> Result<Overview>;
}

#[derive(Clone)]
pub(crate) struct TerminalRpcImpl {
    pub shared: Shared,
}

#[async_trait]
impl TerminalRpc for TerminalRpcImpl {
    fn get_overview(&self, refresh: Option<u32>) -> Result<Overview> {
        let refresh = refresh
            .and_then(RefreshKind::from_bits)
            .unwrap_or(RefreshKind::NOTHING);
        let sys = self.get_sys_info(refresh)?;
        let mining = self.get_mining_info(refresh)?;

        Ok(Overview { sys, mining })
    }
}

impl TerminalRpcImpl {
    fn get_mining_info(&self, refresh: RefreshKind) -> Result<MiningInfo> {
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
        let (difficulty, _overflow) = compact_to_target(current_epoch_ext.compact_target());
        Ok(MiningInfo {
            difficulty,
            // We use previous_epoch_hash_rate to approximate the full network hash power,
            // simplifying the calculation process.
            hash_rate: current_epoch_ext.previous_epoch_hash_rate().to_owned(),
        })
    }

    fn get_sys_info(&self, refresh: RefreshKind) -> Result<SysInfo> {
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
        Ok(sys_info)
    }
}
