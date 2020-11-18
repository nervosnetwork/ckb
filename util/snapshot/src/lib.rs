//! TODO(doc): @zhangsoledad
use arc_swap::{ArcSwap, Guard};
use ckb_chain_spec::consensus::Consensus;
use ckb_db::{
    iter::{DBIter, IteratorMode},
    Col, DBPinnableSlice,
};
use ckb_error::Error;
use ckb_proposal_table::ProposalView;
use ckb_reward_calculator::RewardCalculator;
use ckb_store::{ChainStore, StoreCache, StoreSnapshot};
use ckb_traits::{BlockMedianTimeContext, HeaderProvider};
use ckb_types::core::error::OutPointError;
use ckb_types::{
    core::{
        cell::{CellProvider, CellStatus, HeaderChecker},
        BlockNumber, BlockReward, EpochExt, HeaderView,
    },
    packed::{Byte32, OutPoint, Script},
    U256,
};
use std::sync::Arc;

/// TODO(doc): @zhangsoledad
pub struct SnapshotMgr {
    inner: ArcSwap<Snapshot>,
}

impl SnapshotMgr {
    /// TODO(doc): @zhangsoledad
    pub fn new(snapshot: Arc<Snapshot>) -> Self {
        SnapshotMgr {
            inner: ArcSwap::new(snapshot),
        }
    }

    /// TODO(doc): @zhangsoledad
    pub fn load(&self) -> Guard<Arc<Snapshot>> {
        self.inner.load()
    }

    /// TODO(doc): @zhangsoledad
    pub fn store(&self, snapshot: Arc<Snapshot>) {
        self.inner.store(snapshot);
    }
}

/// TODO(doc): @zhangsoledad
// A snapshot captures a point-in-time view of the DB at the time it's created
//
//                   yes —— new snapshot
//                   /                    \
//    tip —— change?                        SnapshotMgr swap
//                  \                      /
//                   no —— refresh snapshot
pub struct Snapshot {
    tip_header: HeaderView,
    total_difficulty: U256,
    epoch_ext: EpochExt,
    store: StoreSnapshot,
    proposals: ProposalView,
    consensus: Arc<Consensus>,
}

impl Snapshot {
    /// TODO(doc): @zhangsoledad
    // New snapshot created after tip change
    pub fn new(
        tip_header: HeaderView,
        total_difficulty: U256,
        epoch_ext: EpochExt,
        store: StoreSnapshot,
        proposals: ProposalView,
        consensus: Arc<Consensus>,
    ) -> Snapshot {
        Snapshot {
            tip_header,
            total_difficulty,
            epoch_ext,
            store,
            proposals,
            consensus,
        }
    }

    /// TODO(doc): @zhangsoledad
    // Refreshing on block commit is necessary operation, even tip remains unchanged.
    // when node relayed compact block,if some uncles were not available from receiver's local sources,
    // in GetBlockTransactions/BlockTransactions roundtrip, node will need access block data of uncles.
    pub fn refresh(&self, store: StoreSnapshot) -> Snapshot {
        Snapshot {
            store,
            tip_header: self.tip_header.clone(),
            total_difficulty: self.total_difficulty.clone(),
            epoch_ext: self.epoch_ext.clone(),
            proposals: self.proposals.clone(),
            consensus: Arc::clone(&self.consensus),
        }
    }

    /// TODO(doc): @zhangsoledad
    pub fn tip_header(&self) -> &HeaderView {
        &self.tip_header
    }

    /// TODO(doc): @zhangsoledad
    pub fn tip_number(&self) -> BlockNumber {
        self.tip_header.number()
    }

    /// TODO(doc): @zhangsoledad
    pub fn tip_hash(&self) -> Byte32 {
        self.tip_header.hash()
    }

    /// TODO(doc): @zhangsoledad
    pub fn epoch_ext(&self) -> &EpochExt {
        &self.epoch_ext
    }

    /// TODO(doc): @zhangsoledad
    pub fn consensus(&self) -> &Consensus {
        &self.consensus
    }

    /// TODO(doc): @zhangsoledad
    pub fn cloned_consensus(&self) -> Arc<Consensus> {
        Arc::clone(&self.consensus)
    }

    /// TODO(doc): @zhangsoledad
    pub fn proposals(&self) -> &ProposalView {
        &self.proposals
    }

    /// TODO(doc): @zhangsoledad
    pub fn total_difficulty(&self) -> &U256 {
        &self.total_difficulty
    }

    /// TODO(doc): @zhangsoledad
    pub fn finalize_block_reward(
        &self,
        parent: &HeaderView,
    ) -> Result<(Script, BlockReward), Error> {
        RewardCalculator::new(self.consensus(), self).block_reward_to_finalize(parent)
    }
}

impl<'a> ChainStore<'a> for Snapshot {
    type Vector = DBPinnableSlice<'a>;

    fn cache(&'a self) -> Option<&'a StoreCache> {
        self.store.cache()
    }

    fn get(&'a self, col: Col, key: &[u8]) -> Option<Self::Vector> {
        self.store.get(col, key)
    }

    fn get_iter(&self, col: Col, mode: IteratorMode) -> DBIter {
        self.store.get_iter(col, mode)
    }

    fn get_tip_header(&self) -> Option<HeaderView> {
        Some(self.tip_header.clone())
    }

    fn get_current_epoch_ext(&'a self) -> Option<EpochExt> {
        Some(self.epoch_ext.clone())
    }
}

impl CellProvider for Snapshot {
    fn cell(&self, out_point: &OutPoint, with_data: bool) -> CellStatus {
        self.store.cell_provider().cell(out_point, with_data)
    }
}

impl HeaderChecker for Snapshot {
    fn check_valid(&self, block_hash: &Byte32) -> Result<(), Error> {
        match self.get_block_header(block_hash) {
            Some(header) => {
                let threshold =
                    self.consensus.cellbase_maturity().to_rational() + header.epoch().to_rational();
                let current = self.tip_header().epoch().to_rational();
                if current < threshold {
                    Err(OutPointError::ImmatureHeader(block_hash.clone()).into())
                } else {
                    Ok(())
                }
            }
            None => Err(OutPointError::InvalidHeader(block_hash.clone()).into()),
        }
    }
}

impl BlockMedianTimeContext for Snapshot {
    fn median_block_count(&self) -> u64 {
        self.consensus.median_time_block_count() as u64
    }
}

impl HeaderProvider for Snapshot {
    fn get_header(&self, hash: &Byte32) -> Option<HeaderView> {
        self.store.get_block_header(hash)
    }
}
