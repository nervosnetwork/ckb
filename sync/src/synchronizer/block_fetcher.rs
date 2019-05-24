use crate::synchronizer::{BlockStatus, Synchronizer};
use crate::types::HeaderView;
use crate::{BLOCK_DOWNLOAD_WINDOW, MAX_BLOCKS_IN_TRANSIT_PER_PEER, PER_FETCH_BLOCK_LIMIT};
use ckb_core::header::Header;
use ckb_network::PeerIndex;
use ckb_store::ChainStore;
use log::{debug, trace};
use numext_fixed_hash::H256;
use numext_fixed_uint::U256;
use std::cmp;

pub struct BlockFetcher<CS: ChainStore> {
    synchronizer: Synchronizer<CS>,
    peer: PeerIndex,
    tip_header: Header,
    total_difficulty: U256,
}

impl<CS> BlockFetcher<CS>
where
    CS: ChainStore,
{
    pub fn new(synchronizer: Synchronizer<CS>, peer: PeerIndex) -> Self {
        let (tip_header, total_difficulty) = {
            let chain_state = synchronizer.shared.lock_chain_state();
            (
                chain_state.tip_header().to_owned(),
                chain_state.total_difficulty().to_owned(),
            )
        };
        BlockFetcher {
            peer,
            synchronizer,
            tip_header,
            total_difficulty,
        }
    }
    pub fn reached_inflight_limit(&self) -> bool {
        let inflight = self.synchronizer.peers.blocks_inflight.read();

        // Can't download any more from this peer
        inflight.peer_inflight_count(&self.peer) >= MAX_BLOCKS_IN_TRANSIT_PER_PEER
    }

    pub fn is_better_chain(&self, header: &HeaderView) -> bool {
        *header.total_difficulty() >= self.total_difficulty
    }

    pub fn peer_best_known_header(&self) -> Option<HeaderView> {
        self.synchronizer.peers.get_best_known_header(self.peer)
    }

    pub fn last_common_header(&self, best: &HeaderView) -> Option<Header> {
        let last_common_header = {
            if let Some(header) = self.synchronizer.peers().get_last_common_header(self.peer) {
                Some(header.clone())
            } else if best.number() < self.tip_header.number() {
                let last_common_hash = self.synchronizer.shared.block_hash(best.number())?;
                self.synchronizer.shared.block_header(&last_common_hash)
            } else {
                Some(self.tip_header.clone())
            }
        }?;

        let fixed_last_common_header = self
            .synchronizer
            .shared
            .last_common_ancestor(&last_common_header, &best.inner())?;

        if fixed_last_common_header != last_common_header {
            self.synchronizer
                .peers()
                .set_last_common_header(self.peer, fixed_last_common_header.clone());
        }

        Some(fixed_last_common_header)
    }

    // this peer's tip is wherethe the ancestor of global_best_known_header
    pub fn is_known_best(&self, header: &HeaderView) -> bool {
        let global_best_known_header = self.synchronizer.shared.best_known_header();
        if let Some(ancestor) = self
            .synchronizer
            .shared
            .get_ancestor(&global_best_known_header.hash(), header.number())
        {
            if &ancestor != header.inner() {
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
        trace!(target: "sync", "[block downloader] BlockFetcher process");

        if self.reached_inflight_limit() {
            trace!(
                target: "sync",
                "[block downloader] inflight count reach limit, can't download any more from peer {}",
                self.peer
            );
            return None;
        }

        let best_known_header = match self.peer_best_known_header() {
            Some(best_known_header) => best_known_header,
            _ => {
                trace!(
                    target: "sync",
                    "[block downloader] peer_best_known_header not found peer={}",
                    self.peer
                );
                return None;
            }
        };

        // This peer has nothing interesting.
        if !self.is_better_chain(&best_known_header) {
            trace!(
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
        let fixed_last_common_header = self.last_common_header(&best_known_header)?;

        if &fixed_last_common_header == best_known_header.inner() {
            trace!(
                target: "sync",
                "[block downloader] fixed_last_common_header == best_known_header"
            );
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
        let mut index_height = fixed_last_common_header.number();
        // Read up to 128, get_ancestor may be as expensive
        // as iterating over ~100 entries anyway.
        let max_height = cmp::min(max_height, index_height + PER_FETCH_BLOCK_LIMIT as u64);
        let mut fetch = Vec::with_capacity(PER_FETCH_BLOCK_LIMIT);

        {
            let mut inflight = self.synchronizer.peers.blocks_inflight.write();
            let count = MAX_BLOCKS_IN_TRANSIT_PER_PEER
                .saturating_sub(inflight.peer_inflight_count(&self.peer));

            while index_height < max_height && fetch.len() < count {
                index_height += 1;
                let to_fetch = self
                    .synchronizer
                    .shared
                    .get_ancestor(&best_known_header.hash(), index_height)?;
                let to_fetch_hash = to_fetch.hash();

                let block_status = self.synchronizer.get_block_status(to_fetch_hash);
                if block_status == BlockStatus::VALID_MASK
                    && inflight.insert(self.peer, to_fetch_hash.to_owned())
                {
                    trace!(
                        target: "sync", "[Synchronizer] inflight insert {:?}------------{:x}",
                        to_fetch.number(),
                        to_fetch_hash
                    );
                    fetch.push(to_fetch_hash.to_owned());
                }
            }
        }
        Some(fetch)
    }
}
