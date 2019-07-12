use crate::error::RPCError;
use ckb_jsonrpc_types::{BannedAddress, Node, NodeAddress, Timestamp, Unsigned};
use ckb_network::NetworkController;
use faketime::unix_time_as_millis;
use jsonrpc_core::Result;
use jsonrpc_derive::rpc;

const MAX_ADDRS: usize = 50;
const DEFAULT_BAN_DURATION: u64 = 24 * 60 * 60 * 1000; // 1 day

#[rpc]
pub trait NetworkRpc {
    // curl -d '{"id": 2, "jsonrpc": "2.0", "method":"local_node_info","params": []}' -H 'content-type:application/json' 'http://localhost:8114'
    #[rpc(name = "local_node_info")]
    fn local_node_info(&self) -> Result<Node>;

    // curl -d '{"id": 2, "jsonrpc": "2.0", "method":"get_peers","params": []}' -H 'content-type:application/json' 'http://localhost:8114'
    #[rpc(name = "get_peers")]
    fn get_peers(&self) -> Result<Vec<Node>>;

    // curl -d '{"id": 2, "jsonrpc": "2.0", "method":"get_banned_addresses","params": []}' -H 'content-type:application/json' 'http://localhost:8114'
    #[rpc(name = "get_banned_addresses")]
    fn get_banned_addresses(&self) -> Result<Vec<BannedAddress>>;

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
                .external_urls(MAX_ADDRS)
                .into_iter()
                .map(|(address, score)| NodeAddress {
                    address,
                    score: Unsigned(u64::from(score)),
                })
                .collect(),
        })
    }

    fn get_peers(&self) -> Result<Vec<Node>> {
        let peers = self.network_controller.connected_peers();
        Ok(peers
            .into_iter()
            .map(|(peer_id, peer, addresses)| Node {
                is_outbound: Some(peer.is_outbound()),
                version: peer
                    .identify_info
                    .map(|info| info.client_version)
                    .unwrap_or_else(|| "unknown".to_string()),
                node_id: peer_id.to_base58(),
                // TODO how to get correct port and score?
                addresses: addresses
                    .into_iter()
                    .map(|(address, score)| NodeAddress {
                        address: address.to_string(),
                        score: Unsigned(u64::from(score)),
                    })
                    .collect(),
            })
            .collect())
    }

    fn get_banned_addresses(&self) -> Result<Vec<BannedAddress>> {
        Ok(self
            .network_controller
            .get_banned_addresses()
            .into_iter()
            .map(|banned| BannedAddress {
                address: banned.address.to_string(),
                ban_until: Timestamp(banned.ban_until),
                ban_reason: banned.ban_reason,
                created_at: Timestamp(banned.created_at),
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
                    ban_time.unwrap_or_default().0
                } else {
                    unix_time_as_millis() + ban_time.unwrap_or(Timestamp(DEFAULT_BAN_DURATION)).0
                };
                self.network_controller.insert_ban(
                    ip_network,
                    ban_until,
                    &reason.unwrap_or_default(),
                )
            }
            "delete" => self.network_controller.delete_ban(&ip_network),
            _ => Err(RPCError::custom(
                RPCError::Invalid,
                "invalid command".to_owned(),
            ))?,
        }
        Ok(())
    }
}
