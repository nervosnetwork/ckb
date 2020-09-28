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

/// RPC Module Net for P2P network.
#[rpc(server)]
pub trait NetRpc {
    /// Returns the local node information.
    ///
    /// The local node means the node itself which is serving the RPC.
    ///
    /// ## Examples
    ///
    /// Request
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "method": "local_node_info",
    ///   "params": []
    /// }
    /// ```
    ///
    /// Response
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "result": {
    ///     "active": true,
    ///     "addresses": [
    ///       {
    ///         "address": "/ip4/192.168.0.2/tcp/8112/p2p/QmTRHCdrRtgUzYLNCin69zEvPvLYdxUZLLfLYyHVY3DZAS",
    ///         "score": "0xff"
    ///       },
    ///       {
    ///         "address": "/ip4/0.0.0.0/tcp/8112/p2p/QmTRHCdrRtgUzYLNCin69zEvPvLYdxUZLLfLYyHVY3DZAS",
    ///         "score": "0x1"
    ///       }
    ///     ],
    ///     "connections": "0xb",
    ///     "node_id": "QmTRHCdrRtgUzYLNCin69zEvPvLYdxUZLLfLYyHVY3DZAS",
    ///     "protocols": [
    ///       {
    ///         "id": "0x0",
    ///         "name": "/ckb/ping",
    ///         "support_versions": [
    ///           "0.0.1"
    ///         ]
    ///       },
    ///       {
    ///         "id": "0x1",
    ///         "name": "/ckb/discovery",
    ///         "support_versions": [
    ///           "0.0.1"
    ///         ]
    ///       }
    ///     ],
    ///     "version": "0.34.0 (f37f598 2020-07-17)"
    ///   }
    /// }
    /// ```
    #[rpc(name = "local_node_info")]
    fn local_node_info(&self) -> Result<LocalNode>;

    /// Returns the connected peers' information.
    ///
    /// ## Examples
    ///
    /// Request
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "method": "get_peers",
    ///   "params": []
    /// }
    /// ```
    ///
    /// Response
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "result": [
    ///     {
    ///       "addresses": [
    ///         {
    ///           "address": "/ip6/::ffff:18.185.102.19/tcp/8115/p2p/QmXwUgF48ULy6hkgfqrEwEfuHW7WyWyWauueRDAYQHNDfN",
    ///           "score": "0x64"
    ///         },
    ///         {
    ///           "address": "/ip4/18.185.102.19/tcp/8115/p2p/QmXwUgF48ULy6hkgfqrEwEfuHW7WyWyWauueRDAYQHNDfN",
    ///           "score": "0x64"
    ///         }
    ///       ],
    ///       "connected_duration": "0x2f",
    ///       "is_outbound": true,
    ///       "last_ping_duration": "0x1a",
    ///       "node_id": "QmXwUgF48ULy6hkgfqrEwEfuHW7WyWyWauueRDAYQHNDfN",
    ///       "protocols": [
    ///         {
    ///           "id": "0x4",
    ///           "version": "0.0.1"
    ///         },
    ///         {
    ///           "id": "0x2",
    ///           "version": "0.0.1"
    ///         },
    ///         {
    ///           "id": "0x1",
    ///           "version": "0.0.1"
    ///         },
    ///         {
    ///           "id": "0x64",
    ///           "version": "1"
    ///         },
    ///         {
    ///           "id": "0x6e",
    ///           "version": "1"
    ///         },
    ///         {
    ///           "id": "0x66",
    ///           "version": "1"
    ///         },
    ///         {
    ///           "id": "0x65",
    ///           "version": "1"
    ///         },
    ///         {
    ///           "id": "0x0",
    ///           "version": "0.0.1"
    ///         }
    ///       ],
    ///       "sync_state": {
    ///         "best_known_header_hash": null,
    ///         "best_known_header_number": null,
    ///         "can_fetch_count": "0x80",
    ///         "inflight_count": "0xa",
    ///         "last_common_header_hash": null,
    ///         "last_common_header_number": null,
    ///         "unknown_header_list_size": "0x20"
    ///       },
    ///       "version": "0.34.0 (f37f598 2020-07-17)"
    ///     },
    ///     {
    ///       "addresses": [
    ///         {
    ///           "address": "/ip4/174.80.182.60/tcp/52965/p2p/QmVTMd7SEXfxS5p4EEM5ykTe1DwWWVewEM3NwjLY242vr2",
    ///           "score": "0x1"
    ///         }
    ///       ],
    ///       "connected_duration": "0x95",
    ///       "is_outbound": true,
    ///       "last_ping_duration": "0x41",
    ///       "node_id": "QmSrkzhdBMmfCGx8tQGwgXxzBg8kLtX8qMcqECMuKWsxDV",
    ///       "protocols": [
    ///         {
    ///           "id": "0x0",
    ///           "version": "0.0.1"
    ///         },
    ///         {
    ///           "id": "0x2",
    ///           "version": "0.0.1"
    ///         },
    ///         {
    ///           "id": "0x6e",
    ///           "version": "1"
    ///         },
    ///         {
    ///           "id": "0x66",
    ///           "version": "1"
    ///         },
    ///         {
    ///           "id": "0x1",
    ///           "version": "0.0.1"
    ///         },
    ///         {
    ///           "id": "0x65",
    ///           "version": "1"
    ///         },
    ///         {
    ///           "id": "0x64",
    ///           "version": "1"
    ///         },
    ///         {
    ///           "id": "0x4",
    ///           "version": "0.0.1"
    ///         }
    ///       ],
    ///       "sync_state": {
    ///         "best_known_header_hash": "0x2157c72b3eddd41a7a14c361173cd22ef27d7e0a29eda2e511ee0b3598c0b895",
    ///         "best_known_header_number": "0xdb835",
    ///         "can_fetch_count": "0x80",
    ///         "inflight_count": "0xa",
    ///         "last_common_header_hash": "0xc63026bd881d880bb142c855dc8153187543245f0a94391c831c75df31f263c4",
    ///         "last_common_header_number": "0x4dc08",
    ///         "unknown_header_list_size": "0x1f"
    ///       },
    ///       "version": "0.30.1 (5cc1b75 2020-03-23)"
    ///     }
    ///   ]
    /// }
    /// ```
    #[rpc(name = "get_peers")]
    fn get_peers(&self) -> Result<Vec<RemoteNode>>;

    /// Returns all banned IPs/Subnets.
    ///
    /// ## Examples
    ///
    /// Request
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "method": "get_banned_addresses",
    ///   "params": []
    /// }
    /// ```
    ///
    /// Response
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "result": [
    ///     {
    ///       "address": "192.168.0.2/32",
    ///       "ban_reason": "",
    ///       "ban_until": "0x1ac89236180",
    ///       "created_at": "0x16bde533338"
    ///     }
    ///   ]
    /// }
    /// ```
    #[rpc(name = "get_banned_addresses")]
    fn get_banned_addresses(&self) -> Result<Vec<BannedAddr>>;

    /// Clears all banned IPs/Subnets.
    ///
    /// ## Examples
    ///
    /// Request
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "method": "clear_banned_addresses",
    ///   "params": []
    /// }
    /// ```
    ///
    /// Response
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "result": null
    /// }
    /// ```
    #[rpc(name = "clear_banned_addresses")]
    fn clear_banned_addresses(&self) -> Result<()>;

    /// Inserts or deletes an IP/Subnet from the banned list
    ///
    /// ## Params
    ///
    /// * `address` - The IP/Subnet with an optional netmask (default is /32 = single IP). Examples:
    ///     * "192.168.0.2" bans a single IP
    ///     * "192.168.0.0/24" bans IP from "192.168.0.0" to "192.168.0.255".
    /// * `command` - `insert` to insert an IP/Subnet to the list, `delete` to delete an IP/Subnet from the list.
    /// * `ban_time` - Time in milliseconds how long (or until when if [absolute] is set) the IP is banned, optional parameter, null means using the default time of 24h
    /// * `absolute` - If set, the `ban_time` must be an absolute timestamp in milliseconds since epoch, optional parameter.
    /// * `reason` - Ban reason, optional parameter.
    ///
    /// ## Errors
    ///
    /// * [`InvalidParams (-32602)`](../enum.RPCError.html#variant.InvalidParams)
    ///     * Expected `address` to be a valid IP address with an optional netmask.
    ///     * Expected `command` to be in the list [insert, delete].
    ///
    /// ## Examples
    ///
    /// Request
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "method": "set_ban",
    ///   "params": [
    ///     "192.168.0.2",
    ///     "insert",
    ///     "0x1ac89236180",
    ///     true,
    ///     "set_ban example"
    ///   ]
    /// }
    /// ```
    ///
    /// Response
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "result": null
    /// }
    /// ```
    #[rpc(name = "set_ban")]
    fn set_ban(
        &self,
        address: String,
        command: String,
        ban_time: Option<Timestamp>,
        absolute: Option<bool>,
        reason: Option<String>,
    ) -> Result<()>;

    /// Returns chain synchronization state of this node.
    ///
    /// ## Examples
    ///
    /// Request
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "method": "sync_state",
    ///   "params": []
    /// }
    /// ```
    ///
    /// Response
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "result": {
    ///     "best_known_block_number": "0x400",
    ///     "best_known_block_timestamp": "0x5cd2b117",
    ///     "fast_time": "0x3e8",
    ///     "ibd": true,
    ///     "inflight_blocks_count": "0x0",
    ///     "low_time": "0x5dc",
    ///     "normal_time": "0x4e2",
    ///     "orphan_blocks_count": "0x0"
    ///   }
    /// }
    /// ```
    #[rpc(name = "sync_state")]
    fn sync_state(&self) -> Result<SyncState>;

    /// Disable/enable all p2p network activity
    ///
    /// ## Params
    ///
    /// * `state` - true to enable networking, false to disable
    ///
    /// ## Examples
    ///
    /// Request
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "method": "set_network_active",
    ///   "params": [
    ///     false
    ///   ]
    /// }
    /// ```
    ///
    /// Response
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "result": null
    /// }
    /// ```
    #[rpc(name = "set_network_active")]
    fn set_network_active(&self, state: bool) -> Result<()>;

    /// Attempts to add a node to the peers list and try connecting to it.
    ///
    /// ## Params
    ///
    /// * `peer_id` - The node id of the node.
    /// * `address` - The address of the node.
    ///
    /// The full P2P address is usually displayed as `address/peer_id`, for example in the log
    ///
    /// ```text
    /// 2020-09-16 15:31:35.191 +08:00 NetworkRuntime INFO ckb_network::network
    ///   Listen on address: /ip4/192.168.2.100/tcp/8114/QmUsZHPbjjzU627UZFt4k8j6ycEcNvXRnVGxCPKqwbAfQS
    /// ```
    ///
    /// And in RPC `local_node_info`:
    ///
    /// ```json
    /// {
    ///   "addresses": [
    ///     {
    ///       "address": "/ip4/192.168.2.100/tcp/8114/QmUsZHPbjjzU627UZFt4k8j6ycEcNvXRnVGxCPKqwbAfQS",
    ///       "score": "0xff"
    ///     }
    ///   ]
    /// }
    /// ```
    ///
    /// In both of these examples,
    ///
    /// * `peer_id` is `QmUsZHPbjjzU627UZFt4k8j6ycEcNvXRnVGxCPKqwbAfQS`,
    /// * and `address` is `/ip4/192.168.2.100/tcp/8114`
    ///
    /// ## Examples
    ///
    /// Request
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "method": "add_node",
    ///   "params": [
    ///     "QmUsZHPbjjzU627UZFt4k8j6ycEcNvXRnVGxCPKqwbAfQS",
    ///     "/ip4/192.168.2.100/tcp/8114"
    ///   ]
    /// }
    /// ```
    ///
    /// Response
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "result": null
    /// }
    /// ```
    #[rpc(name = "add_node")]
    fn add_node(&self, peer_id: String, address: String) -> Result<()>;

    /// Attempts to remove a node from the peers list and try disconnecting from it.
    ///
    /// ## Params
    ///
    /// * `peer_id` - The peer id of the node.
    ///
    /// This is the last part of a full P2P address. For example, in address
    /// "/ip4/192.168.2.100/tcp/8114/QmUsZHPbjjzU627UZFt4k8j6ycEcNvXRnVGxCPKqwbAfQS", the `peer_id`
    /// is `QmUsZHPbjjzU627UZFt4k8j6ycEcNvXRnVGxCPKqwbAfQS`.
    ///
    /// ## Examples
    ///
    /// Request
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "method": "remove_node",
    ///   "params": [
    ///     "QmUsZHPbjjzU627UZFt4k8j6ycEcNvXRnVGxCPKqwbAfQS"
    ///   ]
    /// }
    /// ```
    ///
    /// Response
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "result": null
    /// }
    /// ```
    #[rpc(name = "remove_node")]
    fn remove_node(&self, peer_id: String) -> Result<()>;

    /// Requests that a ping is sent to all connected peers, to measure ping time.
    ///
    /// ## Examples
    ///
    /// Requests
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "method": "ping_peers",
    ///   "params": []
    /// }
    /// ```
    ///
    /// Response
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "result": null
    /// }
    /// ```
    #[rpc(name = "ping_peers")]
    fn ping_peers(&self) -> Result<()>;
}

pub(crate) struct NetRpcImpl {
    pub network_controller: NetworkController,
    pub sync_shared: Arc<SyncShared>,
}

impl NetRpc for NetRpcImpl {
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

    fn clear_banned_addresses(&self) -> Result<()> {
        self.network_controller.clear_banned_addrs();
        Ok(())
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
