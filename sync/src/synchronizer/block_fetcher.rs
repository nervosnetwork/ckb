use crate::block_status::BlockStatus;
use crate::synchronizer::Synchronizer;
use crate::types::{HeaderView, SyncSnapshot};
use crate::MAX_BLOCKS_IN_TRANSIT_PER_PEER;
use ckb_logger::{debug, trace, metric};
use ckb_network::PeerIndex;
use ckb_store::ChainStore;
use ckb_types::{core, packed};
use faketime::unix_time_as_millis;

pub struct BlockFetcher {
    synchronizer: Synchronizer,
    peer: PeerIndex,
    snapshot: SyncSnapshot,
}

impl BlockFetcher {
    pub fn new(synchronizer: Synchronizer, peer: PeerIndex) -> Self {
        let snapshot = synchronizer.shared.snapshot();
        BlockFetcher {
            peer,
            synchronizer,
            snapshot,
        }
    }
    pub fn reached_inflight_limit(&self) -> bool {
        let inflight = self.synchronizer.shared().state().read_inflight_blocks();

        // Can't download any more from this peer
        inflight.peer_inflight_count(self.peer) >= MAX_BLOCKS_IN_TRANSIT_PER_PEER
    }

    pub fn is_better_chain(&self, header: &HeaderView) -> bool {
        header.is_better_than(&self.snapshot.total_difficulty())
    }

    pub fn peer_best_known_header(&self) -> Option<HeaderView> {
        self.synchronizer.peers().get_best_known_header(self.peer)
    }

    pub fn last_common_header(&self, best: &HeaderView) -> Option<core::HeaderView> {
        let last_common_header = {
            if let Some(header) = self.synchronizer.peers().get_last_common_header(self.peer) {
                Some(header)
            // Bootstrap quickly by guessing a parent of our best tip is the forking point.
            // Guessing wrong in either direction is not a problem.
            } else if best.number() < self.snapshot.tip_header().number() {
                let last_common_hash = self.snapshot.store().get_block_hash(best.number())?;
                self.snapshot.store().get_block_header(&last_common_hash)
            } else {
                Some(self.snapshot.tip_header())
            }
        }?;

        // If the peer reorganized, our previous last_common_header may not be an ancestor
        // of its current tip anymore. Go back enough to fix that.
        let fixed_last_common_header = self
            .snapshot
            .last_common_ancestor(&last_common_header, &best.inner())?;

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
                self.snapshot.total_difficulty()
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
        let mut fetch = Vec::with_capacity(MAX_BLOCKS_IN_TRANSIT_PER_PEER);

        let timestamp = unix_time_as_millis();
        let mut steps = 0;
        {
            let mut inflight = self.synchronizer.shared().state().write_inflight_blocks();
            let count = MAX_BLOCKS_IN_TRANSIT_PER_PEER
                .saturating_sub(inflight.peer_inflight_count(self.peer));

            while fetch.len() < count {
                index_height += 1;
                if index_height > best_known_header.number() {
                    break;
                }

                steps += 1;
                let to_fetch = self
                    .snapshot
                    .get_ancestor(&best_known_header.hash(), index_height)?;

                // NOTE: Filtering `BLOCK_STORED` but not `BLOCK_RECEIVED`, is for avoiding
                // stopping synchronization even when orphan_pool maintains dirty items by bugs.
                if self
                    .snapshot
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
        metric!({
            "topic": "fetch_blocks",
            "fields": { "elapsed": unix_time_as_millis().saturating_sub(timestamp), "steps": steps }
        });
        Some(fetch)
    }
}
