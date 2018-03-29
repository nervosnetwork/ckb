use bigint::H256;
use multiaddr::Multiaddr;
use nervos_time::now_ms;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Default)]
pub struct Peers {
    all: HashSet<Multiaddr>,
    unuseful: HashSet<Multiaddr>,
    idle_for_headers: HashSet<Multiaddr>,
    idle_for_blocks: HashSet<Multiaddr>,
    headers_requests: HashSet<Multiaddr>,
    blocks_requests: HashMap<Multiaddr, BlocksRequest>,
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
    pub fn all_peers(&self) -> &HashSet<Multiaddr> {
        &self.all
    }

    /// Get useful peers
    pub fn useful_peers(&self) -> Vec<Multiaddr> {
        self.all.difference(&self.unuseful).cloned().collect()
    }

    /// Get idle peers for headers request.
    pub fn idle_peers_for_headers(&self) -> &HashSet<Multiaddr> {
        &self.idle_for_headers
    }

    /// Get idle peers for blocks request.
    pub fn idle_peers_for_blocks(&self) -> &HashSet<Multiaddr> {
        &self.idle_for_blocks
    }

    /// Mark peer as useful.
    pub fn as_useful_peer(&mut self, addr: &Multiaddr) {
        self.all.insert(addr.clone());
        self.unuseful.remove(addr);
        self.idle_for_headers.insert(addr.clone());
        self.idle_for_blocks.insert(addr.clone());
    }

    /// Mark peer as unuseful.
    pub fn as_unuseful_peer(&mut self, addr: &Multiaddr) {
        debug_assert!(!self.blocks_requests.contains_key(addr));

        self.all.insert(addr.clone());
        self.unuseful.insert(addr.clone());
        self.idle_for_headers.remove(addr);
        self.idle_for_blocks.remove(addr);
    }

    /// Headers been requested from peer.
    pub fn on_headers_requested(&mut self, addr: &Multiaddr) {
        if !self.all.contains(addr) {
            self.as_unuseful_peer(addr);
        }

        self.idle_for_headers.remove(addr);
        self.headers_requests.replace(addr.clone());
    }

    /// Headers received from peer.
    pub fn on_headers_received(&mut self, addr: &Multiaddr) {
        self.headers_requests.remove(addr);
        // we only ask for new headers when peer is also not asked for blocks
        // => only insert to idle queue if no active blocks requests
        if !self.blocks_requests.contains_key(addr) {
            self.idle_for_headers.insert(addr.clone());
        }
    }

    /// Blocks have been requested from peer.
    pub fn on_blocks_requested(&mut self, addr: &Multiaddr, blocks_hashes: &[H256]) {
        if !self.all.contains(addr) {
            self.as_unuseful_peer(addr);
        }
        self.unuseful.remove(addr);
        self.idle_for_blocks.remove(addr);

        if !self.blocks_requests.contains_key(addr) {
            self.blocks_requests
                .insert(addr.clone(), BlocksRequest::new());
        }
        self.blocks_requests
            .get_mut(addr)
            .expect("inserted one")
            .blocks
            .extend(blocks_hashes.iter().cloned());
    }

    pub fn on_block_received(&mut self, addr: &Multiaddr, block_hash: &H256) {
        if let Some(blocks_request) = self.blocks_requests.get_mut(addr) {
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
        self.blocks_requests.remove(addr);
        self.idle_for_blocks.insert(addr.clone());
        // also mark as available for headers request if not yet
        if !self.headers_requests.contains(addr) {
            self.idle_for_headers.insert(addr.clone());
        }
    }
}
