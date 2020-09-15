use crate::error::RPCError;
use ckb_jsonrpc_types::{
    BannedAddr, LocalNode, LocalNodeProtocol, NodeAddress, PeerSyncState, RemoteNode,
    RemoteNodeProtocol, SyncState, Timestamp,
};
use ckb_network::{MultiaddrExt, NetworkController};
use ckb_sync::SyncShared;
use faketime::unix_time_as_millis;
use jsonrpc_core::Result;
use jsonrpc_derive::rpc;
use std::{collections::HashMap, sync::Arc};

const MAX_ADDRS: usize = 50;
const DEFAULT_BAN_DURATION: u64 = 24 * 60 * 60 * 1000; // 1 day

#[rpc(server)]
pub trait NetworkRpc {
    // curl -d '{"id": 2, "jsonrpc": "2.0", "method":"local_node_info","params": []}' -H 'content-type:application/json' 'http://localhost:8114'
    #[rpc(name = "local_node_info")]
    fn local_node_info(&self) -> Result<LocalNode>;

    // curl -d '{"id": 2, "jsonrpc": "2.0", "method":"get_peers","params": []}' -H 'content-type:application/json' 'http://localhost:8114'
    #[rpc(name = "get_peers")]
    fn get_peers(&self) -> Result<Vec<RemoteNode>>;

    // curl -d '{"id": 2, "jsonrpc": "2.0", "method":"get_banned_addresses","params": []}' -H 'content-type:application/json' 'http://localhost:8114'
    #[rpc(name = "get_banned_addresses")]
    fn get_banned_addresses(&self) -> Result<Vec<BannedAddr>>;

    // curl -d '{"id": 2, "jsonrpc": "2.0", "method":"set_ban","params": ["192.168.0.0/24", "insert"]}' -H 'content-type:application/json' 'http://localhost:8114'
    #[rpc(name = "set_ban")]
    fn set_ban(
        &self,
        address: String,
        command: String,
        ban_time: Option<Timestamp>,
        absolute: Option<bool>,
        reason: Option<String>,
    ) -> Result<()>;

    // curl -d '{"id": 2, "jsonrpc": "2.0", "method":"sync_state","params": []}' -H 'content-type:application/json' 'http://localhost:8114'
    #[rpc(name = "sync_state")]
    fn sync_state(&self) -> Result<SyncState>;

    // curl -d '{"id": 2, "jsonrpc": "2.0", "method":"set_network_active","params": [false]}' -H 'content-type:application/json' 'http://localhost:8114'
    #[rpc(name = "set_network_active")]
    fn set_network_active(&self, state: bool) -> Result<()>;

    // curl -d '{"id": 2, "jsonrpc": "2.0", "method":"add_node","params": ["QmUsZHPbjjzU627UZFt4k8j6ycEcNvXRnVGxCPKqwbAfQS", "/ip4/192.168.2.100/tcp/30002"]}' -H 'content-type:application/json' 'http://localhost:8114'
    #[rpc(name = "add_node")]
    fn add_node(&self, peer_id: String, address: String) -> Result<()>;

    // curl -d '{"id": 2, "jsonrpc": "2.0", "method":"remove_node","params": ["QmUsZHPbjjzU627UZFt4k8j6ycEcNvXRnVGxCPKqwbAfQS"]}' -H 'content-type:application/json' 'http://localhost:8114'
    #[rpc(name = "remove_node")]
    fn remove_node(&self, peer_id: String) -> Result<()>;

    #[rpc(name = "ping_peers")]
    fn ping_peers(&self) -> Result<()>;
}

pub(crate) struct NetworkRpcImpl {
    pub network_controller: NetworkController,
    pub sync_shared: Arc<SyncShared>,
}

impl NetworkRpc for NetworkRpcImpl {
    fn local_node_info(&self) -> Result<LocalNode> {
        Ok(LocalNode {
            version: self.network_controller.version().to_owned(),
            node_id: self.network_controller.node_id(),
            active: self.network_controller.is_active(),
            addresses: self
                .network_controller
                .public_urls(MAX_ADDRS)
                .into_iter()
                .map(|(address, score)| NodeAddress {
                    address,
                    score: u64::from(score).into(),
                })
                .collect(),
            protocols: self
                .network_controller
                .protocols()
                .into_iter()
                .map(|(protocol_id, name, support_versions)| LocalNodeProtocol {
                    id: (protocol_id.value() as u64).into(),
                    name,
                    support_versions,
                })
                .collect::<Vec<_>>(),
            connections: (self.network_controller.connected_peers().len() as u64).into(),
        })
    }

    fn get_peers(&self) -> Result<Vec<RemoteNode>> {
        let peers: Vec<RemoteNode> = self
            .network_controller
            .connected_peers()
            .iter()
            .map(|(peer_index, peer)| {
                let peer_id = peer.peer_id.clone();
                let mut addresses = vec![&peer.connected_addr];
                addresses.extend(peer.listened_addrs.iter());

                let mut node_addresses = HashMap::with_capacity(addresses.len());
                for address in addresses {
                    if let Ok(ip_port) = address.extract_ip_addr() {
                        let p2p_address = address.attach_p2p(&peer_id).expect("always ok");
                        let score = self
                            .network_controller
                            .addr_info(&ip_port)
                            .map(|addr_info| addr_info.score)
                            .unwrap_or(1);
                        let non_negative_score = if score > 0 { score as u64 } else { 0 };
                        node_addresses.insert(
                            ip_port,
                            NodeAddress {
                                address: p2p_address.to_string(),
                                score: non_negative_score.into(),
                            },
                        );
                    }
                }

                let inflight_blocks = self.sync_shared.state().read_inflight_blocks();
                RemoteNode {
                    is_outbound: peer.is_outbound(),
                    version: peer
                        .identify_info
                        .as_ref()
                        .map(|info| info.client_version.clone())
                        .unwrap_or_else(|| "unknown".to_string()),
                    node_id: peer_id.to_base58(),
                    addresses: node_addresses.values().cloned().collect(),
                    connected_duration: peer.connected_time.elapsed().as_secs().into(),
                    last_ping_duration: peer
                        .ping
                        .map(|duration| (duration.as_millis() as u64).into()),
                    sync_state: self
                        .sync_shared
                        .state()
                        .peers()
                        .state
                        .read()
                        .get(&peer_index)
                        .map(|state| PeerSyncState {
                            best_known_header_hash: state
                                .best_known_header
                                .as_ref()
                                .map(|header| header.hash().into()),
                            best_known_header_number: state
                                .best_known_header
                                .as_ref()
                                .map(|header| header.number().into()),
                            last_common_header_hash: state
                                .last_common_header
                                .as_ref()
                                .map(|header| header.hash().into()),
                            last_common_header_number: state
                                .last_common_header
                                .as_ref()
                                .map(|header| header.number().into()),
                            unknown_header_list_size: (state.unknown_header_list.len() as u64)
                                .into(),
                            inflight_count: (inflight_blocks.peer_inflight_count(*peer_index)
                                as u64)
                                .into(),
                            can_fetch_count: (inflight_blocks.peer_can_fetch_count(*peer_index)
                                as u64)
                                .into(),
                        }),
                    protocols: peer
                        .protocols
                        .iter()
                        .map(|(protocol_id, protocol_version)| RemoteNodeProtocol {
                            id: (protocol_id.value() as u64).into(),
                            version: protocol_version.clone(),
                        })
                        .collect(),
                }
            })
            .collect();

        Ok(peers)
    }

    fn get_banned_addresses(&self) -> Result<Vec<BannedAddr>> {
        Ok(self
            .network_controller
            .get_banned_addrs()
            .into_iter()
            .map(|banned| BannedAddr {
                address: banned.address.to_string(),
                ban_until: banned.ban_until.into(),
                ban_reason: banned.ban_reason,
                created_at: banned.created_at.into(),
            })
            .collect())
    }

    fn set_ban(
        &self,
        address: String,
        command: String,
        ban_time: Option<Timestamp>,
        absolute: Option<bool>,
        reason: Option<String>,
    ) -> Result<()> {
        let ip_network = address.parse().map_err(|_| {
            RPCError::invalid_params(format!(
                "Expected `params[0]` to be a valid IP address, got {}",
                address
            ))
        })?;

        match command.as_ref() {
            "insert" => {
                let ban_until = if absolute.unwrap_or(false) {
                    ban_time.unwrap_or_default().into()
                } else {
                    unix_time_as_millis()
                        + ban_time
                            .unwrap_or_else(|| DEFAULT_BAN_DURATION.into())
                            .value()
                };
                self.network_controller
                    .ban(ip_network, ban_until, reason.unwrap_or_default())
                    .map_err(RPCError::ckb_internal_error)
            }
            "delete" => {
                self.network_controller.unban(&ip_network);
                Ok(())
            }
            _ => Err(RPCError::invalid_params(format!(
                "Expected `params[1]` to be in the list [insert, delete], got {}",
                address,
            ))),
        }
    }

    fn sync_state(&self) -> Result<SyncState> {
        let chain = self.sync_shared.active_chain();
        let state = chain.shared().state();
        let (fast_time, normal_time, low_time) = state.read_inflight_blocks().division_point();
        let best_known = state.shared_best_header();
        let sync_state = SyncState {
            ibd: chain.is_initial_block_download(),
            best_known_block_number: best_known.number().into(),
            best_known_block_timestamp: best_known.timestamp().into(),
            orphan_blocks_count: (state.orphan_pool().len() as u64).into(),
            inflight_blocks_count: (state.read_inflight_blocks().total_inflight_count() as u64)
                .into(),
            fast_time: fast_time.into(),
            normal_time: normal_time.into(),
            low_time: low_time.into(),
        };

        Ok(sync_state)
    }

    fn set_network_active(&self, state: bool) -> Result<()> {
        self.network_controller.set_active(state);
        Ok(())
    }

    fn add_node(&self, peer_id: String, address: String) -> Result<()> {
        self.network_controller.add_node(
            &peer_id.parse().expect("invalid peer_id"),
            address.parse().expect("invalid address"),
        );
        Ok(())
    }

    fn remove_node(&self, peer_id: String) -> Result<()> {
        self.network_controller
            .remove_node(&peer_id.parse().expect("invalid peer_id"));
        Ok(())
    }

    fn ping_peers(&self) -> Result<()> {
        self.network_controller.ping_peers();
        Ok(())
    }
}
