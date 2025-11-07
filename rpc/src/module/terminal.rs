use crate::error::RPCError;
use async_trait::async_trait;
use ckb_dao_utils::extract_dao_data;
use ckb_db_schema::COLUMN_CELL;
use ckb_jsonrpc_types::{
    CellsInfo, Disk, DiskUsage, Global, MiningInfo, Network, Overview, SysInfo, TerminalPoolInfo,
};
use ckb_logger::error;
use ckb_network::NetworkController;
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
    pub network_controller: NetworkController,
}

#[async_trait]
impl TerminalRpc for TerminalRpcImpl {
    fn get_overview(&self, refresh: Option<u32>) -> Result<Overview> {
        let refresh = refresh
            .and_then(RefreshKind::from_bits)
            .unwrap_or(RefreshKind::NOTHING);
        let sys = self.get_sys_info(refresh)?;
        let mining = self.get_mining_info(refresh)?;
        let pool = self.get_tx_pool_info(refresh)?;
        let cells = self.get_cells_info(refresh)?;

        Ok(Overview {
            sys,
            cells,
            mining,
            pool,
            version: self.network_controller.version().to_owned(),
        })
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

    fn get_tx_pool_info(&self, refresh: RefreshKind) -> Result<TerminalPoolInfo> {
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

        let tx_pool_info = TerminalPoolInfo {
            pending: (info.pending_size as u64).into(),
            proposed: (info.proposed_size as u64).into(),
            orphan: (info.orphan_size as u64).into(),
            committing: (block_template.transactions.len() as u64).into(),
            total_tx_size: (info.total_tx_size as u64).into(),
            total_tx_cycles: info.total_tx_cycles.into(),
            last_txs_updated_at: info.last_txs_updated_at.into(),
            max_tx_pool_size: info.max_tx_pool_size.into(),
        };

        Ok(tx_pool_info)
    }

    fn get_cells_info(&self, refresh: RefreshKind) -> Result<CellsInfo> {
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
        Ok(cells_info)
    }
}
