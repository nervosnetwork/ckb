//! Rocksdb snapshot wrapper

use arc_swap::{ArcSwap, Guard};
use ckb_chain_spec::consensus::{Consensus, ConsensusProvider};
use ckb_db::{
    iter::{DBIter, IteratorMode},
    DBPinnableSlice,
};
use ckb_db_schema::Col;
use ckb_freezer::Freezer;
use ckb_proposal_table::ProposalView;
use ckb_store::{ChainStore, StoreCache, StoreSnapshot};
use ckb_traits::HeaderProvider;
use ckb_types::core::error::OutPointError;
use ckb_types::{
    core::{
        cell::{CellChecker, CellProvider, CellStatus, HeaderChecker},
        BlockNumber, EpochExt, HeaderView,
    },
    packed::{Byte32, OutPoint},
    U256,
};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

/// An Atomic wrapper for Snapshot
pub struct SnapshotMgr {
    inner: ArcSwap<Snapshot>,
}

impl SnapshotMgr {
    /// Create new SnapshotMgr
    pub fn new(snapshot: Arc<Snapshot>) -> Self {
        SnapshotMgr {
            inner: ArcSwap::new(snapshot),
        }
    }

    /// Provides a temporary borrow of snapshot
    pub fn load(&self) -> Guard<Arc<Snapshot>> {
        self.inner.load()
    }

    /// Replaces the snapshot inside this instance.
    pub fn store(&self, snapshot: Arc<Snapshot>) {
        self.inner.store(snapshot);
    }
}

/// A snapshot captures a point-in-time view of the DB at the time it's created
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

impl Hash for Snapshot {
    fn hash<H: Hasher>(&self, state: &mut H) {
        Hash::hash(&self.tip_header, state);
    }
}

impl PartialEq for Snapshot {
    fn eq(&self, other: &Self) -> bool {
        self.tip_header == other.tip_header
    }
}

impl Eq for Snapshot {}

impl ::std::fmt::Debug for Snapshot {
    fn fmt(&self, f: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {
        write!(f, "Snapshot {{ tip_hash: {} }}", self.tip_header.hash())
    }
}

impl Snapshot {
    /// New snapshot created after tip change
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

    /// Refreshing on block commit is necessary operation, even tip remains unchanged.
    /// when node relayed compact block,if some uncles were not available from receiver's local sources,
    /// in GetBlockTransactions/BlockTransactions roundtrip, node will need access block data of uncles.
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

    /// Return reference of tip header
    pub fn tip_header(&self) -> &HeaderView {
        &self.tip_header
    }

    /// Return tip header number
    pub fn tip_number(&self) -> BlockNumber {
        self.tip_header.number()
    }

    /// Return tip header hash
    pub fn tip_hash(&self) -> Byte32 {
        self.tip_header.hash()
    }

    /// Return current epoch information
    pub fn epoch_ext(&self) -> &EpochExt {
        &self.epoch_ext
    }

    /// Return reference of `Consensus`
    pub fn consensus(&self) -> &Consensus {
        &self.consensus
    }

    /// Makes a clone of the `Arc<Consensus>`
    pub fn cloned_consensus(&self) -> Arc<Consensus> {
        Arc::clone(&self.consensus)
    }

    /// Return reference of proposals view
    pub fn proposals(&self) -> &ProposalView {
        &self.proposals
    }

    /// Return current best chain total_difficulty
    pub fn total_difficulty(&self) -> &U256 {
        &self.total_difficulty
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

    fn freezer(&'a self) -> Option<&'a Freezer> {
        self.store.freezer()
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
    fn cell(&self, out_point: &OutPoint, eager_load: bool) -> CellStatus {
        match self.get_cell(out_point) {
            Some(mut cell_meta) => {
                if eager_load {
                    if let Some((data, data_hash)) = self.get_cell_data(out_point) {
                        cell_meta.mem_cell_data = Some(data);
                        cell_meta.mem_cell_data_hash = Some(data_hash);
                    }
                }
                CellStatus::live_cell(cell_meta)
            }
            None => CellStatus::Unknown,
        }
    }
}

impl CellChecker for Snapshot {
    fn is_live(&self, out_point: &OutPoint) -> Option<bool> {
        if self.have_cell(out_point) {
            Some(true)
        } else {
            None
        }
    }
}

impl HeaderChecker for Snapshot {
    fn check_valid(&self, block_hash: &Byte32) -> Result<(), OutPointError> {
        if !self.is_main_chain(block_hash) {
            return Err(OutPointError::InvalidHeader(block_hash.clone()));
        }
        self.get_block_header(block_hash)
            .ok_or_else(|| OutPointError::InvalidHeader(block_hash.clone()))?;
        Ok(())
    }
}

impl HeaderProvider for Snapshot {
    fn get_header(&self, hash: &Byte32) -> Option<HeaderView> {
        self.store.get_block_header(hash)
    }
}

impl ConsensusProvider for Snapshot {
    fn get_consensus(&self) -> &Consensus {
        self.consensus()
    }
}
