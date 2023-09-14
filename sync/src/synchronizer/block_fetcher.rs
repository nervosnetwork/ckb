use crate::block_status::BlockStatus;
use crate::synchronizer::Synchronizer;
use crate::types::{ActiveChain, BlockNumberAndHash, HeaderIndex, HeaderIndexView, IBDState};
use ckb_constant::sync::{
    BLOCK_DOWNLOAD_WINDOW, CHECK_POINT_WINDOW, INIT_BLOCKS_IN_TRANSIT_PER_PEER,
};
use ckb_logger::{debug, trace};
use ckb_network::PeerIndex;
use ckb_systemtime::unix_time_as_millis;
use ckb_types::packed;
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

    pub fn peer_best_known_header(&self) -> Option<HeaderIndex> {
        self.synchronizer.peers().get_best_known_header(self.peer)
    }

    pub fn update_last_common_header(
        &self,
        best_known: &BlockNumberAndHash,
    ) -> Option<BlockNumberAndHash> {
        // Bootstrap quickly by guessing an ancestor of our best tip is forking point.
        // Guessing wrong in either direction is not a problem.
        let mut last_common =
            if let Some(header) = self.synchronizer.peers().get_last_common_header(self.peer) {
                header
            } else {
                let tip_header = self.active_chain.tip_header();
                let guess_number = min(tip_header.number(), best_known.number());
                let guess_hash = self.active_chain.get_block_hash(guess_number)?;
                (guess_number, guess_hash).into()
            };

        // If the peer reorganized, our previous last_common_header may not be an ancestor
        // of its current tip anymore. Go back enough to fix that.
        last_common = self
            .active_chain
            .last_common_ancestor(&last_common, best_known)?;

        self.synchronizer
            .peers()
            .set_last_common_header(self.peer, last_common.clone());

        Some(last_common)
    }

    pub fn fetch(self) -> Option<Vec<Vec<packed::Byte32>>> {
        if self.reached_inflight_limit() {
            trace!(
                "[block_fetcher] inflight count reach limit, can't download any more from peer {}",
                self.peer
            );
            return None;
        }

        // Update `best_known_header` based on `unknown_header_list`. It must be involved before
        // our acquiring the newest `best_known_header`.
        if let IBDState::In = self.ibd {
            let state = self.synchronizer.shared.state();
            // unknown list is an ordered list, sorted from highest to lowest,
            // when header hash unknown, break loop is ok
            while let Some(hash) = state.peers().take_unknown_last(self.peer) {
                // Here we need to first try search from headermap, if not, fallback to search from the db.
                // if not search from db, it can stuck here when the headermap may have been removed just as the block was downloaded
                if let Some(header) = self.synchronizer.shared.get_header_index_view(&hash, false) {
                    state
                        .peers()
                        .may_set_best_known_header(self.peer, header.as_header_index());
                } else {
                    state.peers().insert_unknown_header_hash(self.peer, hash);
                    break;
                }
            }
        }

        let best_known = match self.peer_best_known_header() {
            Some(t) => t,
            None => {
                debug!(
                    "peer {} doesn't have best known header, ignore it",
                    self.peer
                );
                return None;
            }
        };
        if !best_known.is_better_than(self.active_chain.total_difficulty()) {
            // Advancing this peer's last_common_header is unnecessary for block-sync mechanism.
            // However, RPC `get_peers`, returns peers information which includes
            // last_common_header, is expected to provide a more realistic picture. Hence here we
            // specially advance this peer's last_common_header at the case of both us on the same
            // active chain.
            if self.active_chain.is_main_chain(&best_known.hash()) {
                self.synchronizer
                    .peers()
                    .set_last_common_header(self.peer, best_known.number_and_hash());
            }

            return None;
        }

        let best_known = best_known.number_and_hash();
        let last_common = self.update_last_common_header(&best_known)?;
        if last_common == best_known {
            return None;
        }

        let state = self.synchronizer.shared().state();
        let mut inflight = state.write_inflight_blocks();
        let mut start = last_common.number() + 1;
        let mut end = min(best_known.number(), start + BLOCK_DOWNLOAD_WINDOW);
        let n_fetch = min(
            end.saturating_sub(start) as usize + 1,
            inflight.peer_can_fetch_count(self.peer),
        );
        let mut fetch = Vec::with_capacity(n_fetch);
        let now = unix_time_as_millis();

        while fetch.len() < n_fetch && start <= end {
            let span = min(end - start + 1, (n_fetch - fetch.len()) as u64);

            // Iterate in range `[start, start+span)` and consider as the next to-fetch candidates.
            let mut header = self
                .active_chain
                .get_ancestor(&best_known.hash(), start + span - 1)?;
            let mut status = self.active_chain.get_block_status(&header.hash());

            // Judge whether we should fetch the target block, neither stored nor in-flighted
            for _ in 0..span {
                let parent_hash = header.parent_hash();
                let hash = header.hash();

                if status.contains(BlockStatus::BLOCK_STORED) {
                    // If the block is stored, its ancestor must on store
                    // So we can skip the search of this space directly
                    self.synchronizer
                        .peers()
                        .set_last_common_header(self.peer, header.number_and_hash());
                    end = min(best_known.number(), header.number() + BLOCK_DOWNLOAD_WINDOW);
                    break;
                } else if status.contains(BlockStatus::BLOCK_RECEIVED) {
                    // Do not download repeatedly
                } else if (matches!(self.ibd, IBDState::In)
                    || state.compare_with_pending_compact(&hash, now))
                    && inflight.insert(self.peer, (header.number(), hash).into())
                {
                    fetch.push(header)
                }

                status = self.active_chain.get_block_status(&parent_hash);
                header = self
                    .synchronizer
                    .shared
                    .get_header_index_view(&parent_hash, false)?;
            }

            // Move `start` forward
            start += span;
        }

        // The headers in `fetch` may be unordered. Sort them by number.
        fetch.sort_by_key(|header| header.number());

        let tip = self.active_chain.tip_number();
        let should_mark = fetch.last().map_or(false, |header| {
            header.number().saturating_sub(CHECK_POINT_WINDOW) > tip
        });
        if should_mark {
            inflight.mark_slow_block(tip);
        }

        if fetch.is_empty() {
            debug!(
                "[block fetch empty] fixed_last_common_header = {} \
                best_known_header = {}, tip = {}, inflight_len = {}, \
                inflight_state = {:?}",
                last_common.number(),
                best_known.number(),
                tip,
                inflight.total_inflight_count(),
                *inflight
            )
        }

        Some(
            fetch
                .chunks(INIT_BLOCKS_IN_TRANSIT_PER_PEER)
                .map(|headers| headers.iter().map(HeaderIndexView::hash).collect())
                .collect(),
        )
    }
}
