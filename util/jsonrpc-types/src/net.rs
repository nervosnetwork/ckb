use crate::{BlockNumber, Byte32, Timestamp, Uint64};
use serde::{Deserialize, Serialize};

/// The information of the node itself.
///
/// ## Examples
///
/// ```
/// # serde_json::from_str::<ckb_jsonrpc_types::LocalNode>(r#"
/// {
///   "active": true,
///   "addresses": [
///     {
///       "address": "/ip4/192.168.0.2/tcp/8112/p2p/QmTRHCdrRtgUzYLNCin69zEvPvLYdxUZLLfLYyHVY3DZAS",
///       "score": "0xff"
///     },
///     {
///       "address": "/ip4/0.0.0.0/tcp/8112/p2p/QmTRHCdrRtgUzYLNCin69zEvPvLYdxUZLLfLYyHVY3DZAS",
///       "score": "0x1"
///     }
///   ],
///   "connections": "0xb",
///   "node_id": "QmTRHCdrRtgUzYLNCin69zEvPvLYdxUZLLfLYyHVY3DZAS",
///   "protocols": [
///     {
///       "id": "0x0",
///       "name": "/ckb/ping",
///       "support_versions": [
///         "0.0.1"
///       ]
///     },
///     {
///       "id": "0x1",
///       "name": "/ckb/discovery",
///       "support_versions": [
///         "0.0.1"
///       ]
///     }
///   ],
///   "version": "0.34.0 (f37f598 2020-07-17)"
/// }
/// # "#).unwrap();
/// ```
#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct LocalNode {
    /// CKB node version.
    ///
    /// Example: "version": "0.34.0 (f37f598 2020-07-17)"
    pub version: String,
    /// The unique node ID derived from the p2p private key.
    ///
    /// The private key is generated randomly on the first boot.
    pub node_id: String,
    /// Whether this node is active.
    ///
    /// An inactive node ignores incoming p2p messages and drops outgoing messages.
    pub active: bool,
    /// P2P addresses of this node.
    ///
    /// A node can have multiple addresses.
    pub addresses: Vec<NodeAddress>,
    /// Supported protocols.
    pub protocols: Vec<LocalNodeProtocol>,
    /// Count of currently connected peers.
    pub connections: Uint64,
}

/// The information of a P2P protocol that is supported by the local node.
#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct LocalNodeProtocol {
    /// Unique protocol ID.
    pub id: Uint64,
    /// Readable protocol name.
    pub name: String,
    /// Supported versions.
    ///
    /// See [Semantic Version](https://semver.org/) about how to specify a version.
    pub support_versions: Vec<String>,
}

/// Information of a remote node.
///
/// A remote node connects to the local node via the P2P network. It is often called a peer.
///
/// ## Examples
///
/// ```
/// # serde_json::from_str::<ckb_jsonrpc_types::RemoteNode>(r#"
/// {
///   "addresses": [
///     {
///       "address": "/ip6/::ffff:18.185.102.19/tcp/8115/p2p/QmXwUgF48ULy6hkgfqrEwEfuHW7WyWyWauueRDAYQHNDfN",
///       "score": "0x64"
///     },
///     {
///       "address": "/ip4/18.185.102.19/tcp/8115/p2p/QmXwUgF48ULy6hkgfqrEwEfuHW7WyWyWauueRDAYQHNDfN",
///       "score": "0x64"
///     }
///   ],
///   "connected_duration": "0x2f",
///   "is_outbound": true,
///   "last_ping_duration": "0x1a",
///   "node_id": "QmXwUgF48ULy6hkgfqrEwEfuHW7WyWyWauueRDAYQHNDfN",
///   "protocols": [
///     {
///       "id": "0x4",
///       "version": "0.0.1"
///     },
///     {
///       "id": "0x2",
///       "version": "0.0.1"
///     },
///     {
///       "id": "0x1",
///       "version": "0.0.1"
///     },
///     {
///       "id": "0x64",
///       "version": "1"
///     },
///     {
///       "id": "0x6e",
///       "version": "1"
///     },
///     {
///       "id": "0x66",
///       "version": "1"
///     },
///     {
///       "id": "0x65",
///       "version": "1"
///     },
///     {
///       "id": "0x0",
///       "version": "0.0.1"
///     }
///   ],
///   "sync_state": {
///     "best_known_header_hash": null,
///     "best_known_header_number": null,
///     "can_fetch_count": "0x80",
///     "inflight_count": "0xa",
///     "last_common_header_hash": null,
///     "last_common_header_number": null,
///     "unknown_header_list_size": "0x20"
///   },
///   "version": "0.34.0 (f37f598 2020-07-17)"
/// }
/// # "#).unwrap();
/// ```
#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct RemoteNode {
    /// The remote node version.
    pub version: String,
    /// The remote node ID which is derived from its P2P private key.
    pub node_id: String,
    /// The remote node addresses.
    pub addresses: Vec<NodeAddress>,
    /// Whether this is an outbound remote node.
    ///
    /// If the connection is established by the local node, `is_outbound` is true.
    pub is_outbound: bool,
    /// Elapsed time in seconds since the remote node is connected.
    pub connected_duration: Uint64,
    /// Elapsed time in milliseconds since receiving the ping response from this remote node.
    ///
    /// Null means no ping responses have been received yet.
    pub last_ping_duration: Option<Uint64>,
    /// Chain synchronization state.
    ///
    /// Null means chain sync has not started with this remote node yet.
    pub sync_state: Option<PeerSyncState>,
    /// Active protocols.
    ///
    /// CKB uses Tentacle multiplexed network framework. Multiple protocols are running
    /// simultaneously in the connection.
    pub protocols: Vec<RemoteNodeProtocol>,
}

/// The information about an active running protocol.
#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct RemoteNodeProtocol {
    /// Unique protocol ID.
    pub id: Uint64,
    /// Active protocol version.
    pub version: String,
}

/// The chain synchronization state between the local node and a remote node.
#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct PeerSyncState {
    /// Best known header hash of remote peer.
    ///
    /// This is the observed tip of the remote node's canonical chain.
    pub best_known_header_hash: Option<Byte32>,
    /// Best known header number of remote peer
    ///
    /// This is the block number of the block with the hash `best_known_header_hash`.
    pub best_known_header_number: Option<Uint64>,
    /// Last common header hash of remote peer.
    ///
    /// This is the common ancestor of the local node canonical chain tip and the block
    /// `best_known_header_hash`.
    pub last_common_header_hash: Option<Byte32>,
    /// Last common header number of remote peer.
    ///
    /// This is the block number of the block with the hash `last_common_header_hash`.
    pub last_common_header_number: Option<Uint64>,
    /// The total size of unknown header list.
    ///
    /// **Deprecated**: this is an internal state and will be removed in a future release.
    pub unknown_header_list_size: Uint64,
    /// The count of concurrency downloading blocks.
    pub inflight_count: Uint64,
    /// The count of blocks are available for concurrency download.
    pub can_fetch_count: Uint64,
}

/// Node P2P address and score.
#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct NodeAddress {
    /// P2P address.
    ///
    /// This is the same address used in the whitelist in ckb.toml.
    ///
    /// Example: "/ip4/192.168.0.2/tcp/8112/p2p/QmTRHCdrRtgUzYLNCin69zEvPvLYdxUZLLfLYyHVY3DZAS"
    pub address: String,
    /// Address score.
    ///
    /// A higher score means a higher probability of a successful connection.
    pub score: Uint64,
}

/// A banned P2P address.
#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct BannedAddr {
    /// The P2P address.
    ///
    /// Example: "/ip4/192.168.0.2/tcp/8112/p2p/QmTRHCdrRtgUzYLNCin69zEvPvLYdxUZLLfLYyHVY3DZAS"
    pub address: String,
    /// The address is banned until this time.
    pub ban_until: Timestamp,
    /// The reason.
    pub ban_reason: String,
    /// When this address is banned.
    pub created_at: Timestamp,
}

/// The overall chain synchronization state of this local node.
#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct SyncState {
    /// Whether the local node is in IBD, Initial Block Download.
    ///
    /// When a node starts and its chain tip timestamp is far behind the wall clock, it will enter
    /// the IBD until it catches up the synchronization.
    ///
    /// During IBD, the local node only synchronizes the chain with one selected remote node and
    /// stops responding to most P2P requests.
    pub ibd: bool,
    /// This is the best known block number observed by the local node from the P2P network.
    ///
    /// The best here means that the block leads a chain which has the best known accumulated
    /// difficulty.
    ///
    /// This can be used to estimate the synchronization progress. If this RPC returns B, and the
    /// RPC `get_tip_block_number` returns T, the node has already synchronized T/B blocks.
    pub best_known_block_number: BlockNumber,
    /// This is timestamp of the same block described in `best_known_block_number`.
    pub best_known_block_timestamp: Timestamp,
    /// Count of orphan blocks the local node has downloaded.
    ///
    /// The local node downloads multiple blocks simultaneously but blocks must be connected
    /// consecutively. If a descendant is downloaded before its ancestors, it becomes an orphan
    /// block.
    ///
    /// If this number is too high, it indicates that block download has stuck at some block.
    pub orphan_blocks_count: Uint64,
    /// Count of downloading blocks.
    pub inflight_blocks_count: Uint64,
    /// The download scheduler's time analysis data, the fast is the 1/3 of the cut-off point, unit ms
    pub fast_time: Uint64,
    /// The download scheduler's time analysis data, the normal is the 4/5 of the cut-off point, unit ms
    pub normal_time: Uint64,
    /// The download scheduler's time analysis data, the low is the 9/10 of the cut-off point, unit ms
    pub low_time: Uint64,
}
