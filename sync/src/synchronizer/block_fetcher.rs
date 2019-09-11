use crate::block_status::BlockStatus;
use crate::synchronizer::Synchronizer;
use crate::types::HeaderView;
use crate::{MAX_BLOCKS_IN_TRANSIT_PER_PEER, PER_FETCH_BLOCK_LIMIT};
use ckb_logger::{debug, trace};
use ckb_network::PeerIndex;
use ckb_store::ChainStore;
use ckb_types::{core, packed, U256};
use std::cmp::min;

pub struct BlockFetcher {
    synchronizer: Synchronizer,
    peer: PeerIndex,
    tip_header: core::HeaderView,
    total_difficulty: U256,
}

impl BlockFetcher {
    pub fn new(synchronizer: Synchronizer, peer: PeerIndex) -> Self {
        let (tip_header, total_difficulty) = {
            let snapshot = synchronizer.shared.snapshot();
            (
                snapshot.tip_header().to_owned(),
                snapshot.total_difficulty().to_owned(),
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
        let inflight = self.synchronizer.shared().read_inflight_blocks();

        // Can't download any more from this peer
        inflight.peer_inflight_count(self.peer) >= MAX_BLOCKS_IN_TRANSIT_PER_PEER
    }

    pub fn is_better_chain(&self, header: &HeaderView) -> bool {
        header.is_better_than(&self.total_difficulty)
    }

    pub fn peer_best_known_header(&self) -> Option<HeaderView> {
        self.synchronizer.peers().get_best_known_header(self.peer)
    }

    pub fn last_common_header(&self, best: &HeaderView) -> Option<core::HeaderView> {
        let last_common_header = {
            if let Some(header) = self.synchronizer.peers().get_last_common_header(self.peer) {
                Some(header)
            } else if best.number() < self.tip_header.number() {
                let last_common_hash = self
                    .synchronizer
                    .shared
                    .store()
                    .get_block_hash(best.number())?;
                self.synchronizer
                    .shared
                    .store()
                    .get_block_header(&last_common_hash)
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

    pub fn fetch(self) -> Option<Vec<packed::Byte32>> {
        trace!("[block downloader] BlockFetcher process");

        if self.reached_inflight_limit() {
            trace!(
                "[block downloader] inflight count reach limit, can't download any more from peer {}",
                self.peer
            );
            return None;
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
                self.total_difficulty
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

        let mut index_height = fixed_last_common_header.number();
        // Read up to 128, get_ancestor may be as expensive
        // as iterating over ~100 entries anyway.
        let mut fetch = Vec::with_capacity(PER_FETCH_BLOCK_LIMIT);

        {
            let mut inflight = self.synchronizer.shared().write_inflight_blocks();
            let count = min(
                MAX_BLOCKS_IN_TRANSIT_PER_PEER
                    .saturating_sub(inflight.peer_inflight_count(self.peer)),
                PER_FETCH_BLOCK_LIMIT,
            );

            while fetch.len() < count {
                index_height += 1;
                if index_height > best_known_header.number() {
                    break;
                }

                let to_fetch = self
                    .synchronizer
                    .shared
                    .get_ancestor(&best_known_header.hash(), index_height)?;
                if self
                    .synchronizer
                    .shared()
                    // NOTE: Filtering `BLOCK_STORED` but not `BLOCK_RECEIVED`, is for avoiding
                    // stopping synchronization even when orphan_pool maintains dirty items by bugs.
                    .contains_block_status(&to_fetch.hash(), BlockStatus::BLOCK_STORED)
                {
                    continue;
                }

                if inflight.insert(self.peer, to_fetch.hash()) {
                    trace!(
                        "[Synchronizer] inflight insert {:?}------------{}",
                        to_fetch.number(),
                        to_fetch.hash(),
                    );
                    fetch.push(to_fetch.hash());
                }
            }
        }
        Some(fetch)
    }
}
