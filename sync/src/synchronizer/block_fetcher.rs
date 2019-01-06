use crate::synchronizer::{BlockStatus, Synchronizer};
use crate::types::HeaderView;
use crate::{
    BLOCK_DOWNLOAD_TIMEOUT, BLOCK_DOWNLOAD_WINDOW, MAX_BLOCKS_IN_TRANSIT_PER_PEER,
    PER_FETCH_BLOCK_LIMIT,
};
use ckb_core::header::Header;
use ckb_network::PeerIndex;
use ckb_shared::index::ChainIndex;
use ckb_shared::shared::ChainProvider;
use ckb_util::{try_option, RwLockUpgradableReadGuard};
use faketime::unix_time_as_millis;
use log::debug;
use numext_fixed_hash::H256;
use numext_fixed_uint::U256;
use std::cmp;

pub struct BlockFetcher<CI: ChainIndex> {
    synchronizer: Synchronizer<CI>,
    peer: PeerIndex,
    tip_header: Header,
    total_difficulty: U256,
}

impl<CI> BlockFetcher<CI>
where
    CI: ChainIndex,
{
    pub fn new(synchronizer: Synchronizer<CI>, peer: PeerIndex) -> Self {
        let (tip_header, total_difficulty) = {
            let chain_state = synchronizer.shared.chain_state().read();
            (
                chain_state.tip_header().clone(),
                chain_state.total_difficulty().clone(),
            )
        };
        BlockFetcher {
            peer,
            synchronizer,
            tip_header,
            total_difficulty,
        }
    }
    pub fn initial_and_check_inflight(&self) -> bool {
        let mut blocks_inflight = self.synchronizer.peers.blocks_inflight.write();
        let inflight = blocks_inflight
            .entry(self.peer)
            .or_insert_with(Default::default);

        if inflight.timestamp < unix_time_as_millis().saturating_sub(BLOCK_DOWNLOAD_TIMEOUT) {
            debug!(target: "sync", "[block downloader] inflight block download timeout");
            inflight.clear();
        }

        // current peer block blocks_inflight reach limit
        if MAX_BLOCKS_IN_TRANSIT_PER_PEER.saturating_sub(inflight.len()) == 0 {
            debug!(target: "sync", "[block downloader] inflight count reach limit");
            true
        } else {
            false
        }
    }

    pub fn is_better_chain(&self, header: &HeaderView) -> bool {
        *header.total_difficulty() >= self.total_difficulty
    }

    pub fn peer_best_known_header(&self) -> Option<HeaderView> {
        self.synchronizer
            .peers
            .best_known_headers
            .read()
            .get(&self.peer)
            .cloned()
    }

    pub fn last_common_header(&self, best: &HeaderView) -> Option<Header> {
        let guard = self
            .synchronizer
            .peers
            .last_common_headers
            .upgradable_read();

        let last_common_header = try_option!(guard.get(&self.peer).cloned().or_else(|| {
            if best.number() < self.tip_header.number() {
                let last_common_hash = self.synchronizer.shared.block_hash(best.number())?;
                self.synchronizer.shared.block_header(&last_common_hash)
            } else {
                Some(self.tip_header.clone())
            }
        }));

        let fixed_last_common_header = self
            .synchronizer
            .last_common_ancestor(&last_common_header, &best.inner())?;

        if fixed_last_common_header != last_common_header {
            let mut write_guard = RwLockUpgradableReadGuard::upgrade(guard);
            write_guard
                .entry(self.peer)
                .and_modify(|last_common_header| {
                    *last_common_header = fixed_last_common_header.clone()
                })
                .or_insert_with(|| fixed_last_common_header.clone());
        }

        Some(fixed_last_common_header)
    }

    // this peer's tip is wherethe the ancestor of global_best_known_header
    pub fn is_known_best(&self, header: &HeaderView) -> bool {
        let global_best_known_header = { self.synchronizer.best_known_header.read().clone() };
        if let Some(ancestor) = self
            .synchronizer
            .get_ancestor(&global_best_known_header.hash(), header.number())
        {
            if ancestor != header.inner().clone() {
                debug!(
                    target: "sync",
                    "[block downloader] peer best_known_header is not ancestor of global_best_known_header"
                );
                return false;
            }
        } else {
            return false;
        }
        true
    }

    pub fn fetch(self) -> Option<Vec<H256>> {
        debug!(target: "sync", "[block downloader] BlockFetcher process");

        if self.initial_and_check_inflight() {
            debug!(target: "sync", "[block downloader] inflight count reach limit");
            return None;
        }

        let best_known_header = match self.peer_best_known_header() {
            Some(best_known_header) => best_known_header,
            _ => {
                debug!(target: "sync", "[block downloader] peer_best_known_header not found peer={}", self.peer);
                return None;
            }
        };

        // This peer has nothing interesting.
        if !self.is_better_chain(&best_known_header) {
            debug!(
                target: "sync",
                "[block downloader] best_known_header {} chain {}",
                best_known_header.total_difficulty(),
                self.total_difficulty
            );
            return None;
        }

        if !self.is_known_best(&best_known_header) {
            return None;
        }

        // If the peer reorganized, our previous last_common_header may not be an ancestor
        // of its current best_known_header. Go back enough to fix that.
        let fixed_last_common_header = try_option!(self.last_common_header(&best_known_header));

        if fixed_last_common_header == best_known_header.inner().clone() {
            debug!(target: "sync", "[block downloader] fixed_last_common_header == best_known_header");
            return None;
        }

        debug!(
            target: "sync",
            "[block downloader] fixed_last_common_header = {} best_known_header = {}",
            fixed_last_common_header.number(),
            best_known_header.number()
        );

        debug_assert!(best_known_header.number() > fixed_last_common_header.number());

        let window_end = fixed_last_common_header.number() + BLOCK_DOWNLOAD_WINDOW;
        let max_height = cmp::min(window_end + 1, best_known_header.number());

        let mut n_height = fixed_last_common_header.number();
        let mut v_fetch = Vec::with_capacity(PER_FETCH_BLOCK_LIMIT);

        {
            let mut guard = self.synchronizer.peers.blocks_inflight.write();
            let inflight = guard.get_mut(&self.peer).expect("inflight already init");

            while n_height < max_height && v_fetch.len() < PER_FETCH_BLOCK_LIMIT {
                n_height += 1;
                let to_fetch = try_option!(self
                    .synchronizer
                    .get_ancestor(&best_known_header.hash(), n_height));
                let to_fetch_hash = to_fetch.hash();

                let block_status = self.synchronizer.get_block_status(&to_fetch_hash);
                if block_status == BlockStatus::VALID_MASK
                    && inflight.insert(to_fetch_hash.clone().clone())
                {
                    debug!(
                        target: "sync", "[Synchronizer] inflight insert {:?}------------{:?}",
                        to_fetch.number(),
                        to_fetch_hash
                    );
                    v_fetch.push(to_fetch_hash.clone());
                }
            }
        }
        Some(v_fetch)
    }
}
