use crate::error::RPCError;
use ckb_jsonrpc_types::{BannedAddr, Node, NodeAddress, Timestamp};
use ckb_network::{MultiaddrExt, NetworkController};
use faketime::unix_time_as_millis;
use jsonrpc_core::Result;
use jsonrpc_derive::rpc;
use std::collections::HashMap;

const MAX_ADDRS: usize = 50;
const DEFAULT_BAN_DURATION: u64 = 24 * 60 * 60 * 1000; // 1 day

#[rpc(server)]
pub trait NetworkRpc {
    // curl -d '{"id": 2, "jsonrpc": "2.0", "method":"local_node_info","params": []}' -H 'content-type:application/json' 'http://localhost:8114'
    #[rpc(name = "local_node_info")]
    fn local_node_info(&self) -> Result<Node>;

    // curl -d '{"id": 2, "jsonrpc": "2.0", "method":"get_peers","params": []}' -H 'content-type:application/json' 'http://localhost:8114'
    #[rpc(name = "get_peers")]
    fn get_peers(&self) -> Result<Vec<Node>>;

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
}

pub(crate) struct NetworkRpcImpl {
    pub network_controller: NetworkController,
}

impl NetworkRpc for NetworkRpcImpl {
    fn local_node_info(&self) -> Result<Node> {
        Ok(Node {
            version: self.network_controller.node_version().to_string(),
            is_outbound: None,
            node_id: self.network_controller.node_id(),
            addresses: self
                .network_controller
                .public_urls(MAX_ADDRS)
                .into_iter()
                .map(|(address, score)| NodeAddress {
                    address,
                    score: u64::from(score).into(),
                })
                .collect(),
        })
    }

    fn get_peers(&self) -> Result<Vec<Node>> {
        let peers = self.network_controller.connected_peers();
        Ok(peers
            .into_iter()
            .map(|(peer_id, peer)| {
                let mut addresses: HashMap<_, _> = peer
                    .listened_addrs
                    .iter()
                    .filter_map(|addr| {
                        if let Ok((ip_addr, addr)) = addr.extract_ip_addr().and_then(|ip_addr| {
                            addr.attach_p2p(&peer_id).map(|addr| (ip_addr, addr))
                        }) {
                            Some((
                                ip_addr,
                                NodeAddress {
                                    address: addr.to_string(),
                                    score: 1.into(),
                                },
                            ))
                        } else {
                            None
                        }
                    })
                    .collect();
                if peer.is_outbound() {
                    if let Ok(ip_addr) = peer.connected_addr.extract_ip_addr() {
                        addresses.insert(
                            ip_addr,
                            NodeAddress {
                                address: peer.connected_addr.to_string(),
                                score: u64::from(std::u8::MAX).into(),
                            },
                        );
                    }
                }
                let addresses = addresses.values().cloned().collect();
                Node {
                    is_outbound: Some(peer.is_outbound()),
                    version: peer
                        .identify_info
                        .map(|info| info.client_version)
                        .unwrap_or_else(|| "unknown".to_string()),
                    node_id: peer_id.to_base58(),
                    addresses,
                }
            })
            .collect())
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
        let ip_network = address
            .parse()
            .map_err(|_| RPCError::custom(RPCError::Invalid, "invalid address".to_owned()))?;
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
                if let Err(err) =
                    self.network_controller
                        .ban(ip_network, ban_until, reason.unwrap_or_default())
                {
                    return Err(RPCError::custom(
                        RPCError::Invalid,
                        format!("ban address error {}", err),
                    ));
                }
            }
            "delete" => self.network_controller.unban(&ip_network),
            _ => {
                return Err(RPCError::custom(
                    RPCError::Invalid,
                    "invalid command".to_owned(),
                ))
            }
        }
        Ok(())
    }
}
