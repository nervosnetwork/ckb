use bigint::H256;
use ckb_time::now_ms;
use core::block::IndexedBlock;
use fnv::{FnvHashMap, FnvHashSet};
use header_view::HeaderView;
use network::PeerId;
use util::RwLock;

// const BANSCORE: u32 = 100;

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Negotiate {
    pub prefer_headers: bool,
    //     pub want_cmpct: bool,
    //     pub have_witness: bool,
}

#[derive(Clone, Default, Debug, PartialEq)]
pub struct PeerState {
    pub negotiate: Negotiate,
    pub sync_started: bool,
    pub last_block_announcement: Option<u64>, //ms,
}

#[derive(Debug, Default)]
pub struct Peers {
    pub state: RwLock<FnvHashMap<PeerId, PeerState>>,
    pub misbehavior: RwLock<FnvHashMap<PeerId, u32>>,
    pub blocks_inflight: RwLock<FnvHashMap<PeerId, BlocksInflight>>,
    pub best_known_headers: RwLock<FnvHashMap<PeerId, HeaderView>>,
}

#[derive(Debug, Clone)]
pub struct BlocksInflight {
    pub timestamp: u64,
    pub blocks: FnvHashSet<H256>,
}

impl Default for BlocksInflight {
    fn default() -> Self {
        BlocksInflight {
            blocks: FnvHashSet::default(),
            timestamp: now_ms(),
        }
    }
}

impl BlocksInflight {
    pub fn new(blocks_hash: &H256) -> Self {
        let mut blocks = FnvHashSet::default();
        blocks.insert(*blocks_hash);
        BlocksInflight {
            blocks,
            timestamp: now_ms(),
        }
    }

    pub fn len(&self) -> usize {
        self.blocks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn insert(&mut self, hash: H256) -> bool {
        self.blocks.insert(hash)
    }

    pub fn remove(&mut self, hash: &H256) -> bool {
        self.blocks.remove(hash)
    }

    pub fn update_timestamp(&mut self) {
        self.timestamp = now_ms();
    }
}

impl Peers {
    pub fn misbehavior(&self, peer: &PeerId, score: u32) {
        if score == 0 {
            return;
        }

        let mut map = self.misbehavior.write();
        map.entry(*peer)
            .and_modify(|e| *e += score)
            .or_insert_with(|| score);
    }

    pub fn best_known_header(&self, peer: &PeerId) -> Option<HeaderView> {
        self.best_known_headers.read().get(peer).cloned()
    }

    pub fn new_header_received(&self, peer: &PeerId, header_view: &HeaderView) {
        self.best_known_headers
            .write()
            .entry(*peer)
            .and_modify(|hv| {
                if header_view.total_difficulty > hv.total_difficulty
                    || (header_view.total_difficulty == hv.total_difficulty
                        && header_view.header.hash() < hv.header.hash())
                {
                    *hv = header_view.clone();
                }
            })
            .or_insert_with(|| header_view.clone());
    }

    pub fn getheaders_received(&self, peer: &PeerId) {
        self.state
            .write()
            .entry(*peer)
            .or_insert_with(|| PeerState {
                negotiate: Negotiate::default(),
                sync_started: true,
                last_block_announcement: None,
            });
    }

    pub fn disconnected(&self, peer: &PeerId) {
        self.state.write().remove(peer);
        // self.misbehavior.write().remove(peer);
        self.blocks_inflight.write().remove(peer);
        self.best_known_headers.write().remove(peer);
    }

    pub fn block_received(&self, peer: PeerId, block: &IndexedBlock) {
        let mut blocks_inflight = self.blocks_inflight.write();
        blocks_inflight.entry(peer).and_modify(|inflight| {
            inflight.remove(&block.hash());
        });
    }
}
