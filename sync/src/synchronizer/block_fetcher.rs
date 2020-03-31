use crate::block_status::BlockStatus;
use crate::synchronizer::Synchronizer;
use crate::types::{ActiveChain, HeaderView, IBDState};
use crate::BLOCK_DOWNLOAD_WINDOW;
use ckb_logger::{debug, trace};
use ckb_network::PeerIndex;
use ckb_types::{core, packed};
use std::cmp::min;

pub struct BlockFetcher<'a> {
    synchronizer: &'a Synchronizer,
    peer: PeerIndex,
    active_chain: ActiveChain,
    ibd: IBDState,
}

impl<'a> BlockFetcher<'a> {
    pub fn new(synchronizer: &'a Synchronizer, peer: PeerIndex, ibd: IBDState) -> Self {
        let active_chain = synchronizer.shared.active_chain();
        BlockFetcher {
            peer,
            synchronizer,
            active_chain,
            ibd,
        }
    }

    pub fn reached_inflight_limit(&self) -> bool {
        let inflight = self.synchronizer.shared().state().read_inflight_blocks();

        // Can't download any more from this peer
        inflight.peer_can_fetch_count(self.peer) == 0
    }

    pub fn is_better_chain(&self, header: &HeaderView) -> bool {
        header.is_better_than(&self.active_chain.total_difficulty())
    }

    pub fn peer_best_known_header(&self) -> Option<HeaderView> {
        self.synchronizer.peers().get_best_known_header(self.peer)
    }

    pub fn last_common_header(&self, best: &HeaderView) -> Option<core::HeaderView> {
        let tip_header = self.active_chain.tip_header();
        let last_common_header = {
            if let Some(header) = self.synchronizer.peers().get_last_common_header(self.peer) {
                // may reorganized, then it can't be used
                if header.number() > tip_header.number() {
                    Some(tip_header)
                } else {
                    Some(header)
                }
            // Bootstrap quickly by guessing a parent of our best tip is the forking point.
            // Guessing wrong in either direction is not a problem.
            } else if best.number() < tip_header.number() {
                let last_common_hash = self.active_chain.get_block_hash(best.number())?;
                self.active_chain.get_block_header(&last_common_hash)
            } else {
                Some(tip_header)
            }
        }?;

        // If the peer reorganized, our previous last_common_header may not be an ancestor
        // of its current tip anymore. Go back enough to fix that.
        let fixed_last_common_header = self
            .active_chain
            .last_common_ancestor(&last_common_header, &best.inner())?;

        Some(fixed_last_common_header)
    }

    pub fn fetch(self) -> Option<Vec<Vec<packed::Byte32>>> {
        trace!("[block downloader] BlockFetcher process");

        if self.reached_inflight_limit() {
            trace!(
                "[block downloader] inflight count reach limit, can't download any more from peer {}",
                self.peer
            );
            return None;
        }

        if let IBDState::In = self.ibd {
            self.synchronizer
                .shared
                .state()
                .try_update_best_known_with_unknown_header_list(self.peer)
        }

        let best_known_header = match self.peer_best_known_header() {
            Some(best_known_header) => best_known_header,
            _ => {
                trace!(
                    "[block downloader] peer_best_known_header not found peer={}",
                    self.peer
                );
                return None;
            }
        };

        // This peer has nothing interesting.
        if !self.is_better_chain(&best_known_header) {
            trace!(
                "[block downloader] best_known_header {} chain {}",
                best_known_header.total_difficulty(),
                self.active_chain.total_difficulty()
            );
            return None;
        }

        // If the peer reorganized, our previous last_common_header may not be an ancestor
        // of its current best_known_header. Go back enough to fix that.
        let fixed_last_common_header = self.last_common_header(&best_known_header)?;

        if &fixed_last_common_header == best_known_header.inner() {
            trace!("[block downloader] fixed_last_common_header == best_known_header");
            return None;
        }

        debug!(
            "[block downloader] fixed_last_common_header = {} best_known_header = {}",
            fixed_last_common_header.number(),
            best_known_header.number()
        );

        debug_assert!(best_known_header.number() > fixed_last_common_header.number());

        let mut inflight = self.synchronizer.shared().state().write_inflight_blocks();
        let mut start = fixed_last_common_header.number() + 1;
        let end = min(best_known_header.number(), start + BLOCK_DOWNLOAD_WINDOW);
        let n_fetch = min(
            end.saturating_sub(start) as usize + 1,
            inflight.peer_can_fetch_count(self.peer),
        );
        let mut fetch = Vec::with_capacity(n_fetch);

        while fetch.len() < n_fetch && start <= end {
            let span = min(end - start + 1, (n_fetch - fetch.len()) as u64);

            // Iterate in range `[start, start+span)` and consider as the next to-fetch candidates.
            let mut header = self
                .active_chain
                .get_ancestor(&best_known_header.hash(), start + span - 1)?;

            // Judge whether we should fetch the target block, neither stored nor in-flighted
            for _ in 0..span {
                let parent_hash = header.parent_hash();
                let hash = header.hash();
                // NOTE: Filtering `BLOCK_STORED` but not `BLOCK_RECEIVED`, is for avoiding
                // stopping synchronization even when orphan_pool maintains dirty items by bugs.
                let stored = self
                    .active_chain
                    .contains_block_status(&hash, BlockStatus::BLOCK_STORED);
                if stored {
                    // If the block is stored, its ancestor must on store
                    // So we can skip the search of this space directly
                    break;
                } else if inflight.insert(self.peer, hash, header.number()) {
                    fetch.push(header)
                }

                header = self
                    .synchronizer
                    .shared
                    .get_header_view(&parent_hash)?
                    .into_inner();
            }

            // Move `start` forward
            start += span as u64;
        }

        // The headers in `fetch` may be unordered. Sort them by number.
        fetch.sort_by_key(|header| header.number());

        Some(
            fetch
                .chunks(crate::MAX_BLOCKS_IN_TRANSIT_PER_PEER)
                .map(|headers| headers.iter().map(core::HeaderView::hash).collect())
                .collect(),
        )
    }
}
