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

    pub fn update_last_common_header(&self, best_known: &HeaderView) -> Option<core::HeaderView> {
        // Bootstrap quickly by guessing a parent of our best tip is t
        // Guessing wrong in either direction is not a problem.
        let mut last_common =
            if let Some(header) = self.synchronizer.peers().get_last_common_header(self.peer) {
                header
            } else {
                let tip_header = self.active_chain.tip_header();
                let guess_number = min(tip_header.number(), best_known.number());
                let guess_hash = self.active_chain.get_block_hash(guess_number)?;
                self.active_chain.get_block_header(&guess_hash)?
            };

        // If the peer reorganized, our previous last_common_header may not be an ancestor
        // of its current tip anymore. Go back enough to fix that.
        last_common = self
            .active_chain
            .last_common_ancestor(&last_common, &best_known.inner())?;

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
            self.synchronizer
                .shared
                .state()
                .try_update_best_known_with_unknown_header_list(self.peer)
        }

        // This peer has nothing interesting.
        let best_known = self.peer_best_known_header()?;
        if !self.is_better_chain(&best_known) {
            // Advancing this peer's last_common_header is unnecessary for block-sync mechanism.
            // However, RPC `get_peers`, returns peers information which includes
            // last_common_header, is expected to provide a more realistic picture. Hence here we
            // specially advance this peer's last_common_header at the case of both us on the same
            // active chain.
            if self.active_chain.is_main_chain(&best_known.hash()) {
                let last_common = best_known;
                self.synchronizer
                    .peers()
                    .set_last_common_header(self.peer, last_common.into_inner());
            }

            return None;
        }

        let last_common = self.update_last_common_header(&best_known)?;
        if &last_common == best_known.inner() {
            return None;
        }

        let mut inflight = self.synchronizer.shared().state().write_inflight_blocks();
        let mut start = last_common.number() + 1;
        let mut end = min(best_known.number(), start + BLOCK_DOWNLOAD_WINDOW);
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
                        .set_last_common_header(self.peer, header.clone());
                    end = min(best_known.number(), header.number() + BLOCK_DOWNLOAD_WINDOW);
                    break;
                } else if status.contains(BlockStatus::BLOCK_RECEIVED) {
                    // Do not download repeatedly
                } else if inflight.insert(self.peer, (header.number(), hash).into()) {
                    fetch.push(header)
                }

                status = self.active_chain.get_block_status(&parent_hash);
                header = self
                    .synchronizer
                    .shared
                    .get_header_view(
                        &parent_hash,
                        Some(status.contains(BlockStatus::BLOCK_STORED)),
                    )?
                    .into_inner();
            }

            // Move `start` forward
            start += span;
        }

        // The headers in `fetch` may be unordered. Sort them by number.
        fetch.sort_by_key(|header| header.number());

        let tip = self.active_chain.tip_number();
        let should_mark = fetch.last().map_or(false, |header| {
            header.number().saturating_sub(crate::CHECK_POINT_WINDOW) > tip
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
                .chunks(crate::INIT_BLOCKS_IN_TRANSIT_PER_PEER)
                .map(|headers| headers.iter().map(core::HeaderView::hash).collect())
                .collect(),
        )
    }
}
