//! Rocksdb snapshot wrapper

use arc_swap::{ArcSwap, Guard};
use ckb_chain_spec::{
    consensus::{Consensus, ConsensusProvider},
    versionbits::{DeploymentPos, ThresholdState, VersionbitsIndexer},
};
use ckb_db::{
    iter::{DBIter, IteratorMode},
    DBPinnableSlice,
};
use ckb_db_schema::Col;
use ckb_error::{InternalError, InternalErrorKind};
use ckb_freezer::Freezer;
use ckb_merkle_mountain_range::{leaf_index_to_mmr_size, MMRStoreReadOps, Result as MMRResult};
use ckb_proposal_table::ProposalView;
use ckb_store::{ChainStore, StoreCache, StoreSnapshot};
use ckb_traits::{HeaderFields, HeaderFieldsProvider, HeaderProvider};
use ckb_types::{
    core::{
        cell::{CellChecker, CellProvider, CellStatus, HeaderChecker},
        error::OutPointError,
        BlockNumber, EpochExt, HeaderView, TransactionView, Version,
    },
    packed::{Byte32, Bytes, HeaderDigest, OutPoint},
    prelude::{Entity, Pack},
    utilities::merkle_mountain_range::{
        self, hash_out_point_and_status, CellsRootMMR, ChainRootMMR,
    },
    H256, U256,
};
use std::sync::Arc;
use std::{
    collections::HashMap,
    hash::{Hash, Hasher},
};

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

    /// Returns what version a new block should use.
    pub fn compute_versionbits(&self, parent: &HeaderView) -> Option<Version> {
        self.consensus.compute_versionbits(parent, self)
    }

    /// Returns specified softfork active or not
    pub fn versionbits_active(&self, pos: DeploymentPos) -> bool {
        self.consensus
            .versionbits_state(pos, &self.tip_header, self)
            .map(|state| state == ThresholdState::Active)
            .unwrap_or(false)
    }

    /// Returns the chain root MMR for a provided block.
    pub fn chain_root_mmr(&self, block_number: BlockNumber) -> ChainRootMMR<&Self> {
        let mmr_size = leaf_index_to_mmr_size(block_number);
        ChainRootMMR::new(mmr_size, self)
    }

    /// Returns the `CellsRootMMR` struct of the given block number.
    pub fn cells_root_mmr(&self, block_number: BlockNumber) -> CellsRootMMR<CellsRootMMRSnapshot> {
        let s = CellsRootMMRSnapshot::new(self, block_number);
        CellsRootMMR::new(s.mmr_size(), s)
    }

    /// Build dummy extension for current tip.
    pub fn build_dummy_extension(&self) -> Option<Bytes> {
        let lc_activate = self.versionbits_active(DeploymentPos::LightClient);
        let tc_activate = self.versionbits_active(DeploymentPos::CellsCommitments);
        match (lc_activate, tc_activate) {
            (true, true) => {
                let extension = [0u8; 64];
                Some(extension.as_slice().pack())
            }
            (true, false) => {
                let extension = [0u8; 32];
                Some(extension.as_slice().pack())
            }
            (false, true) => {
                let extension = [0u8; 32];
                Some(extension.as_slice().pack())
            }
            (false, false) => None,
        }
    }

    /// Build extension for current tip with given cellbase and txs.
    pub fn build_extension<'a>(
        &self,
        cellbase: &TransactionView,
        txs: impl Iterator<Item = &'a TransactionView>,
    ) -> Result<Option<Bytes>, InternalError> {
        let tip_header = self.tip_header();
        let lc_activate = self.versionbits_active(DeploymentPos::LightClient);
        let tc_activate = self.versionbits_active(DeploymentPos::CellsCommitments);
        match (lc_activate, tc_activate) {
            (true, true) => {
                let chain_root = self
                    .chain_root_mmr(tip_header.number())
                    .get_root()
                    .map_err(|e| InternalErrorKind::MMR.other(e))?;
                let cells_root = self.build_cells_root(cellbase, txs)?;
                let bytes = [chain_root.calc_mmr_hash().as_slice(), cells_root.as_bytes()]
                    .concat()
                    .pack();
                Ok(Some(bytes))
            }
            (true, false) => {
                let chain_root = self
                    .chain_root_mmr(tip_header.number())
                    .get_root()
                    .map_err(|e| InternalErrorKind::MMR.other(e))?;
                let bytes = chain_root.calc_mmr_hash().as_bytes().pack();
                Ok(Some(bytes))
            }
            (false, true) => {
                let cells_root = self.build_cells_root(cellbase, txs)?;
                let bytes = cells_root.as_bytes().pack();
                Ok(Some(bytes))
            }
            (false, false) => Ok(None),
        }
    }

    fn build_cells_root<'a>(
        &self,
        cellbase: &TransactionView,
        txs: impl Iterator<Item = &'a TransactionView>,
    ) -> Result<H256, InternalError> {
        let tip_number = self.tip_header().number();
        let created_by = tip_number + 1;
        let consumed_by = tip_number + 1;
        let mut uncommitted_out_points = HashMap::new();
        let mut cells_root_mmr = self.cells_root_mmr(created_by);

        for out_point in cellbase.output_pts().into_iter() {
            let hash = hash_out_point_and_status(&out_point, created_by, BlockNumber::MAX);
            cells_root_mmr
                .push(hash.clone())
                .map_err(|e| InternalErrorKind::MMR.other(e))?;
        }

        for tx in txs {
            for input in tx.inputs().into_iter() {
                let out_point = input.previous_output();
                let cell_status = self
                    .get_cells_root_mmr_status(&out_point)
                    .or_else(|| uncommitted_out_points.get(&out_point).cloned())
                    .expect("out_point must exist");
                let hash =
                    hash_out_point_and_status(&out_point, cell_status.created_by, consumed_by);
                cells_root_mmr
                    .update(cell_status.mmr_position, hash)
                    .map_err(|e| InternalErrorKind::MMR.other(e))?;
            }

            for out_point in tx.output_pts().into_iter() {
                let hash = hash_out_point_and_status(&out_point, created_by, BlockNumber::MAX);
                let mmr_position = cells_root_mmr
                    .push(hash.clone())
                    .map_err(|e| InternalErrorKind::MMR.other(e))?;
                let cell_status = merkle_mountain_range::CellStatus::new(mmr_position, created_by);
                uncommitted_out_points.insert(out_point, cell_status);
            }
        }
        if cells_root_mmr.is_empty() {
            // cells root mmr may be empty when there is no txs in the block and cellbase has no outputs (block_number < finalization_delay_length)
            Ok(H256([0u8; 32]))
        } else {
            cells_root_mmr
                .get_root()
                .map_err(|e| InternalErrorKind::MMR.other(e))
        }
    }
}

impl ChainStore for Snapshot {
    fn cache(&self) -> Option<&StoreCache> {
        self.store.cache()
    }

    fn get(&self, col: Col, key: &[u8]) -> Option<DBPinnableSlice> {
        self.store.get(col, key)
    }

    fn freezer(&self) -> Option<&Freezer> {
        self.store.freezer()
    }

    fn get_iter(&self, col: Col, mode: IteratorMode) -> DBIter {
        self.store.get_iter(col, mode)
    }

    fn get_tip_header(&self) -> Option<HeaderView> {
        Some(self.tip_header.clone())
    }

    fn get_current_epoch_ext(&self) -> Option<EpochExt> {
        Some(self.epoch_ext.clone())
    }
}

impl VersionbitsIndexer for Snapshot {
    fn block_epoch_index(&self, block_hash: &Byte32) -> Option<Byte32> {
        ChainStore::get_block_epoch_index(self, block_hash)
    }

    fn epoch_ext(&self, index: &Byte32) -> Option<EpochExt> {
        ChainStore::get_epoch_ext(self, index)
    }

    fn block_header(&self, block_hash: &Byte32) -> Option<HeaderView> {
        ChainStore::get_block_header(self, block_hash)
    }

    fn cellbase(&self, block_hash: &Byte32) -> Option<TransactionView> {
        ChainStore::get_cellbase(self, block_hash)
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
        ChainStore::get_block_header(self, block_hash)
            .ok_or_else(|| OutPointError::InvalidHeader(block_hash.clone()))?;
        Ok(())
    }
}

impl HeaderProvider for Snapshot {
    fn get_header(&self, hash: &Byte32) -> Option<HeaderView> {
        self.store.get_block_header(hash)
    }
}

impl HeaderFieldsProvider for Snapshot {
    fn get_header_fields(&self, hash: &Byte32) -> Option<HeaderFields> {
        self.store
            .get_block_header(hash)
            .map(|header| HeaderFields {
                hash: header.hash(),
                number: header.number(),
                epoch: header.epoch(),
                timestamp: header.timestamp(),
                parent_hash: header.parent_hash(),
            })
    }
}

impl ConsensusProvider for Snapshot {
    fn get_consensus(&self) -> &Consensus {
        self.consensus()
    }
}

impl MMRStoreReadOps<HeaderDigest> for &Snapshot {
    fn get(&self, pos: u64) -> MMRResult<Option<HeaderDigest>> {
        Ok(self.store.get_header_digest(pos))
    }
}

/// A snapshot wrapper for cells root MMR
pub struct CellsRootMMRSnapshot<'a> {
    snapshot: &'a Snapshot,
    block_number: BlockNumber,
}

impl<'a> CellsRootMMRSnapshot<'a> {
    fn new(snapshot: &'a Snapshot, block_number: BlockNumber) -> Self {
        CellsRootMMRSnapshot {
            snapshot,
            block_number,
        }
    }

    fn mmr_size(&self) -> u64 {
        self.snapshot
            .store
            .get_cells_root_mmr_size(self.block_number)
    }
}

impl<'a> MMRStoreReadOps<H256> for CellsRootMMRSnapshot<'a> {
    fn get(&self, pos: u64) -> MMRResult<Option<H256>> {
        Ok(self
            .snapshot
            .get_cells_root_mmr_element(pos, self.block_number))
    }
}
