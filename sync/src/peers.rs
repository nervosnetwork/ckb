use bigint::H256;
use nervos_time::now_ms;
use network::protocol::Peer;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Default)]
pub struct Peers {
    all: HashSet<Peer>,
    unuseful: HashSet<Peer>,
    idle_for_headers: HashSet<Peer>,
    idle_for_blocks: HashSet<Peer>,
    headers_requests: HashSet<Peer>,
    blocks_requests: HashMap<Peer, BlocksRequest>,
}

#[derive(Debug, Clone)]
pub struct BlocksRequest {
    pub timestamp: u64,
    pub blocks: HashSet<H256>,
}

impl BlocksRequest {
    pub fn new() -> Self {
        BlocksRequest {
            timestamp: now_ms(),
            blocks: HashSet::new(),
        }
    }

    pub fn set_timestamp(&mut self, timestamp: u64) {
        self.timestamp = timestamp;
    }
}

impl Peers {
    pub fn all_peers(&self) -> &HashSet<Peer> {
        &self.all
    }

    /// Get useful peers
    pub fn useful_peers(&self) -> Vec<Peer> {
        self.all.difference(&self.unuseful).cloned().collect()
    }

    /// Get idle peers for headers request.
    pub fn idle_peers_for_headers(&self) -> &HashSet<Peer> {
        &self.idle_for_headers
    }

    /// Get idle peers for blocks request.
    pub fn idle_peers_for_blocks(&self) -> &HashSet<Peer> {
        &self.idle_for_blocks
    }

    /// Mark peer as useful.
    pub fn as_useful_peer(&mut self, peer: Peer) {
        self.all.insert(peer);
        self.unuseful.remove(&peer);
        self.idle_for_headers.insert(peer);
        self.idle_for_blocks.insert(peer);
    }

    /// Mark peer as unuseful.
    pub fn as_unuseful_peer(&mut self, peer: Peer) {
        debug_assert!(!self.blocks_requests.contains_key(&peer));

        self.all.insert(peer);
        self.unuseful.insert(peer);
        self.idle_for_headers.remove(&peer);
        self.idle_for_blocks.remove(&peer);
    }

    /// Headers been requested from peer.
    pub fn on_headers_requested(&mut self, peer: Peer) {
        if !self.all.contains(&peer) {
            self.as_unuseful_peer(peer);
        }

        self.idle_for_headers.remove(&peer);
        self.headers_requests.replace(peer);
    }

    /// Headers received from peer.
    pub fn on_headers_received(&mut self, peer: Peer) {
        self.headers_requests.remove(&peer);
        // we only ask for new headers when peer is also not asked for blocks
        // => only insert to idle queue if no active blocks requests
        if !self.blocks_requests.contains_key(&peer) {
            self.idle_for_headers.insert(peer);
        }
    }

    /// Blocks have been requested from peer.
    pub fn on_blocks_requested(&mut self, peer: Peer, blocks_hashes: &[H256]) {
        if !self.all.contains(&peer) {
            self.as_unuseful_peer(peer);
        }
        self.unuseful.remove(&peer);
        self.idle_for_blocks.remove(&peer);

        self.blocks_requests
            .entry(peer)
            .or_insert_with(BlocksRequest::new);

        self.blocks_requests
            .get_mut(&peer)
            .expect("inserted one")
            .blocks
            .extend(blocks_hashes.iter().cloned());
    }

    pub fn on_block_received(&mut self, peer: Peer, block_hash: &H256) {
        if let Some(blocks_request) = self.blocks_requests.get_mut(&peer) {
            // if block hasn't been requested => do nothing
            if !blocks_request.blocks.remove(block_hash) {
                return;
            }

            if !blocks_request.blocks.is_empty() {
                blocks_request.set_timestamp(now_ms());
            }
        } else {
            // this peers hasn't been requested for blocks at all
            return;
        }

        // mark this peer as idle for blocks request
        self.blocks_requests.remove(&peer);
        self.idle_for_blocks.insert(peer);
        // also mark as available for headers request if not yet
        if !self.headers_requests.contains(&peer) {
            self.idle_for_headers.insert(peer);
        }
    }
}
