use crate::types::{ActiveChain, IBDState};
use crate::SyncShared;
use ckb_constant::sync::{
    BLOCK_DOWNLOAD_WINDOW, CHECK_POINT_WINDOW, INIT_BLOCKS_IN_TRANSIT_PER_PEER,
};
use ckb_logger::{debug, trace};
use ckb_metrics::HistogramTimer;
use ckb_network::PeerIndex;
use ckb_shared::block_status::BlockStatus;
use ckb_shared::types::{BlockNumberAndHash, HeaderIndex, HeaderIndexView};
use ckb_systemtime::unix_time_as_millis;
use ckb_types::packed;
use std::cmp::min;
use std::sync::Arc;

pub struct BlockFetcher {
    sync_shared: Arc<SyncShared>,
    peer: PeerIndex,
    active_chain: ActiveChain,
    ibd: IBDState,
}

impl BlockFetcher {
    pub fn new(sync_shared: Arc<SyncShared>, peer: PeerIndex, ibd: IBDState) -> Self {
        let active_chain = sync_shared.active_chain();
        BlockFetcher {
            sync_shared,
            peer,
            active_chain,
            ibd,
        }
    }

    pub fn reached_inflight_limit(&self) -> bool {
        let inflight = self.sync_shared.state().read_inflight_blocks();

        // Can't download any more from this peer
        inflight.peer_can_fetch_count(self.peer) == 0
    }

    pub fn peer_best_known_header(&self) -> Option<HeaderIndex> {
        self.sync_shared
            .state()
            .peers()
            .get_best_known_header(self.peer)
    }

    pub fn update_last_common_header(
        &self,
        best_known: &BlockNumberAndHash,
    ) -> Option<BlockNumberAndHash> {
        // Bootstrap quickly by guessing an ancestor of our best tip is forking point.
        // Guessing wrong in either direction is not a problem.
        let mut last_common = if let Some(header) = self
            .sync_shared
            .state()
            .peers()
            .get_last_common_header(self.peer)
        {
            header
        } else {
            let tip_header = self.active_chain.tip_header();
            let guess_number = min(tip_header.number(), best_known.number());
            let guess_hash = self.active_chain.get_block_hash(guess_number)?;
            (guess_number, guess_hash).into()
        };

        // If the peer reorganized, our previous last_common_header may not be an ancestor
        // of its current tip anymore. Go back enough to fix that.
        last_common = {
            let now = std::time::Instant::now();
            let last_common_ancestor = self
                .active_chain
                .last_common_ancestor(&last_common, best_known)?;
            debug!(
                "last_common_ancestor({:?}, {:?})->{:?} cost {:?}",
                last_common,
                best_known,
                last_common_ancestor,
                now.elapsed()
            );
            last_common_ancestor
        };

        self.sync_shared
            .state()
            .peers()
            .set_last_common_header(self.peer, last_common.clone());

        Some(last_common)
    }

    pub fn fetch(self) -> Option<Vec<Vec<packed::Byte32>>> {
        let _trace_timecost: Option<HistogramTimer> = {
            ckb_metrics::handle().map(|handle| handle.ckb_sync_block_fetch_duration.start_timer())
        };

        if self.reached_inflight_limit() {
            trace!(
                "[block_fetcher] inflight count has reached the limit, preventing further downloads from peer {}",
                self.peer
            );
            return None;
        }

        // Update `best_known_header` based on `unknown_header_list`. It must be involved before
        // our acquiring the newest `best_known_header`.
        if let IBDState::In = self.ibd {
            let state = self.sync_shared.state();
            // unknown list is an ordered list, sorted from highest to lowest,
            // when header hash unknown, break loop is ok
            while let Some(hash) = state.peers().take_unknown_last(self.peer) {
                // Here we need to first try search from headermap, if not, fallback to search from the db.
                // if not search from db, it can stuck here when the headermap may have been removed just as the block was downloaded
                if let Some(header) = self.sync_shared.get_header_index_view(&hash, false) {
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
                    "Peer {} doesn't have best known header; ignore it",
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
                self.sync_shared
                    .state()
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

        if matches!(self.ibd, IBDState::In)
            && best_known.number() <= self.active_chain.unverified_tip_number()
        {
            debug!("In IBD mode, Peer {}'s best_known: {} is less or equal than unverified_tip : {}, won't request block from this peer",
                        self.peer,
                        best_known.number(),
                        self.active_chain.unverified_tip_number()
                    );
            return None;
        };

        let state = self.sync_shared.state();

        let mut start = {
            match self.ibd {
                IBDState::In => self.sync_shared.shared().get_unverified_tip().number() + 1,
                IBDState::Out => last_common.number() + 1,
            }
        };
        let mut end = min(best_known.number(), start + BLOCK_DOWNLOAD_WINDOW);
        let n_fetch = min(
            end.saturating_sub(start) as usize + 1,
            state.read_inflight_blocks().peer_can_fetch_count(self.peer),
        );
        let mut fetch = Vec::with_capacity(n_fetch);
        let now = unix_time_as_millis();
        debug!(
            "finding which blocks to fetch, start: {}, end: {}, best_known: {}",
            start,
            end,
            best_known.number(),
        );

        while fetch.len() < n_fetch && start <= end {
            let span = min(end - start + 1, (n_fetch - fetch.len()) as u64);

            // Iterate in range `[start, start+span)` and consider as the next to-fetch candidates.
            let mut header: HeaderIndexView = {
                match self.ibd {
                    IBDState::In => self
                        .active_chain
                        .get_ancestor_with_unverified(&best_known.hash(), start + span - 1),
                    IBDState::Out => self
                        .active_chain
                        .get_ancestor(&best_known.hash(), start + span - 1),
                }
            }?;

            let mut status = self
                .sync_shared
                .active_chain()
                .get_block_status(&header.hash());

            // Judge whether we should fetch the target block, neither stored nor in-flighted
            for _ in 0..span {
                let parent_hash = header.parent_hash();
                let hash = header.hash();

                if status.contains(BlockStatus::BLOCK_STORED) {
                    if status.contains(BlockStatus::BLOCK_VALID) {
                        // If the block is stored, its ancestor must on store
                        // So we can skip the search of this space directly
                        self.sync_shared
                            .state()
                            .peers()
                            .set_last_common_header(self.peer, header.number_and_hash());
                    }

                    end = min(best_known.number(), header.number() + BLOCK_DOWNLOAD_WINDOW);
                    break;
                } else if status.contains(BlockStatus::BLOCK_RECEIVED) {
                    // Do not download repeatedly
                } else if (matches!(self.ibd, IBDState::In)
                    || state.compare_with_pending_compact(&hash, now))
                    && state
                        .write_inflight_blocks()
                        .insert(self.peer, (header.number(), hash).into())
                {
                    debug!(
                        "block: {}-{} added to inflight, block_status: {:?}",
                        header.number(),
                        header.hash(),
                        status
                    );
                    fetch.push(header)
                }

                status = self
                    .sync_shared
                    .active_chain()
                    .get_block_status(&parent_hash);
                header = self
                    .sync_shared
                    .get_header_index_view(&parent_hash, false)?;
            }

            // Move `start` forward
            start += span;
        }

        // The headers in `fetch` may be unordered. Sort them by number.
        fetch.sort_by_key(|header| header.number());

        let tip = self.active_chain.tip_number();
        let unverified_tip = self.active_chain.unverified_tip_number();
        let should_mark = fetch.last().map_or(false, |header| {
            header.number().saturating_sub(CHECK_POINT_WINDOW) > unverified_tip
        });
        if should_mark {
            state
                .write_inflight_blocks()
                .mark_slow_block(unverified_tip);
        }

        if fetch.is_empty() {
            debug!(
                "[block fetch empty] peer-{}, fixed_last_common_header = {} \
                best_known_header = {}, [tip/unverified_tip]: [{}/{}], inflight_len = {}",
                self.peer,
                last_common.number(),
                best_known.number(),
                tip,
                unverified_tip,
                state.read_inflight_blocks().total_inflight_count(),
            );
            trace!(
                "[block fetch empty] peer-{}, inflight_state = {:?}",
                self.peer,
                *state.read_inflight_blocks()
            );
        } else {
            let fetch_head = fetch.first().map_or(0_u64, |v| v.number());
            let fetch_last = fetch.last().map_or(0_u64, |v| v.number());
            let inflight_peer_count = state.read_inflight_blocks().peer_inflight_count(self.peer);
            let inflight_total_count = state.read_inflight_blocks().total_inflight_count();
            debug!(
                "request peer-{} for batch blocks: [{}-{}], batch len:{}, [tip/unverified_tip]: [{}/{}], [peer/total inflight count]: [{} / {}], blocks: {}",
                self.peer,
                fetch_head,
                fetch_last,
                fetch.len(),
                tip,
                self.sync_shared.shared().get_unverified_tip().number(),
                inflight_peer_count,
                inflight_total_count,
                fetch.iter().map(|h| h.number().to_string()).collect::<Vec<_>>().join(","),
                );
        }

        Some(
            fetch
                .chunks(INIT_BLOCKS_IN_TRANSIT_PER_PEER)
                .map(|headers| headers.iter().map(HeaderIndexView::hash).collect())
                .collect(),
        )
    }
}
