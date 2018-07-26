use bigint::H256;
use ckb_chain::chain::ChainProvider;
use core::header::BlockNumber;
use header_view::HeaderView;
use network::PeerId;
use std::cmp;
use synchronizer::Synchronizer;
use {BLOCK_DOWNLOAD_WINDOW, MAX_BLOCKS_IN_TRANSIT_PER_PEER, PER_FETCH_BLOCK_LIMIT};

pub struct BlockFetcher<C> {
    synchronizer: Synchronizer<C>,
    peer: PeerId,
}

impl<C> BlockFetcher<C>
where
    C: ChainProvider,
{
    pub fn new(synchronizer: &Synchronizer<C>, peer: PeerId) -> Self {
        BlockFetcher {
            peer,
            synchronizer: synchronizer.clone(),
        }
    }
    pub fn initial_and_check_inflight(&self) -> bool {
        let mut blocks_inflight = self.synchronizer.peers.blocks_inflight.write();
        let inflight_count = blocks_inflight
            .entry(self.peer)
            .or_insert_with(Default::default)
            .len();

        // current peer block blocks_inflight reach limit
        if MAX_BLOCKS_IN_TRANSIT_PER_PEER.saturating_sub(inflight_count) == 0 {
            debug!(target: "sync", "[block downloader] inflight count reach limit");
            true
        } else {
            false
        }
    }

    pub fn peer_best_known_header(&self) -> Option<HeaderView> {
        self.synchronizer
            .peers
            .best_known_headers
            .read()
            .get(&self.peer)
            .cloned()
    }

    pub fn latest_common_height(&self, header: &HeaderView) -> Option<BlockNumber> {
        let chain_tip = self.synchronizer.chain.tip_header().read();
        //if difficulty of this peer less than our
        if header.total_difficulty < chain_tip.total_difficulty {
            debug!(
                target: "sync",
                "[block downloader] best_known_header {} chain {}",
                header.total_difficulty,
                chain_tip.total_difficulty
            );
            None
        } else {
            Some(cmp::min(header.header.number, chain_tip.header.number))
        }
    }

    // this peer's tip is wherethe the ancestor of global_best_known_header
    pub fn is_known_best(&self, header: &HeaderView) -> bool {
        let global_best_known_header = { self.synchronizer.best_known_header.read().clone() };
        if let Some(ancestor) = self.synchronizer.get_ancestor(
            &global_best_known_header.header.hash(),
            header.header.number,
        ) {
            if ancestor != header.header {
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

        let latest_common_height = try_option!(self.latest_common_height(&best_known_header));

        if !self.is_known_best(&best_known_header) {
            return None;
        }

        let latest_common_hash =
            try_option!(self.synchronizer.chain.block_hash(latest_common_height));
        let latest_common_header =
            try_option!(self.synchronizer.chain.block_header(&latest_common_hash));

        // If the peer reorganized, our previous last_common_header may not be an ancestor
        // of its current best_known_header. Go back enough to fix that.
        let fixed_latest_common_header = try_option!(
            self.synchronizer
                .last_common_ancestor(&latest_common_header, &best_known_header.header)
        );

        if fixed_latest_common_header == best_known_header.header {
            debug!(target: "sync", "[block downloader] fixed_latest_common_header == best_known_header");
            return None;
        }

        debug!(target: "sync", "[block downloader] fixed_latest_common_header = {}", fixed_latest_common_header.number);

        debug_assert!(best_known_header.header.number > fixed_latest_common_header.number);

        let window_end = fixed_latest_common_header.number + BLOCK_DOWNLOAD_WINDOW;
        let max_height = cmp::min(window_end + 1, best_known_header.header.number);

        let mut n_height = fixed_latest_common_header.number;
        let mut v_fetch = Vec::with_capacity(PER_FETCH_BLOCK_LIMIT);

        {
            let mut guard = self.synchronizer.peers.blocks_inflight.write();
            let inflight = guard.get_mut(&self.peer).expect("inflight already init");

            while n_height < max_height && v_fetch.len() < PER_FETCH_BLOCK_LIMIT {
                n_height += 1;
                let to_fetch = try_option!(
                    self.synchronizer
                        .get_ancestor(&best_known_header.header.hash(), n_height)
                );
                let to_fetch_hash = to_fetch.header.hash();

                if inflight.insert(to_fetch_hash) {
                    v_fetch.push(to_fetch_hash);
                }
            }
        }
        Some(v_fetch)
    }
}
