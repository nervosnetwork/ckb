use arc_swap::{ArcSwap, Guard};
use ckb_chain_spec::consensus::Consensus;
use ckb_db::{
    iter::{DBIteratorItem, Direction},
    Col, DBPinnableSlice,
};
use ckb_proposal_table::ProposalView;
use ckb_store::{ChainStore, StoreSnapshot};
use ckb_traits::BlockMedianTimeContext;
use ckb_types::{
    core::{
        cell::{CellProvider, CellStatus, HeaderChecker, UnresolvableError},
        BlockNumber, EpochExt, HeaderView, TransactionMeta,
    },
    packed::{Byte32, OutPoint},
    prelude::*,
    U256,
};
use im::hashmap::HashMap as HamtMap;
use std::sync::Arc;

pub struct SnapshotMgr {
    inner: ArcSwap<Snapshot>,
}

impl SnapshotMgr {
    pub fn new(snapshot: Arc<Snapshot>) -> Self {
        SnapshotMgr {
            inner: ArcSwap::new(snapshot),
        }
    }

    pub fn load(&self) -> Guard<Arc<Snapshot>> {
        self.inner.load()
    }

    pub fn store(&self, snapshot: Arc<Snapshot>) {
        self.inner.store(snapshot);
    }
}

pub struct Snapshot {
    tip_header: HeaderView,
    total_difficulty: U256,
    epoch_ext: EpochExt,
    store: StoreSnapshot,
    cell_set: HamtMap<Byte32, TransactionMeta>,
    proposals: ProposalView,
    consensus: Arc<Consensus>,
}

impl Snapshot {
    pub fn new(
        tip_header: HeaderView,
        total_difficulty: U256,
        epoch_ext: EpochExt,
        store: StoreSnapshot,
        cell_set: HamtMap<Byte32, TransactionMeta>,
        proposals: ProposalView,
        consensus: Arc<Consensus>,
    ) -> Snapshot {
        Snapshot {
            tip_header,
            total_difficulty,
            epoch_ext,
            store,
            cell_set,
            proposals,
            consensus,
        }
    }

    pub fn tip_header(&self) -> &HeaderView {
        &self.tip_header
    }

    pub fn tip_number(&self) -> BlockNumber {
        self.tip_header.number()
    }

    pub fn tip_hash(&self) -> Byte32 {
        self.tip_header.hash()
    }

    pub fn epoch_ext(&self) -> &EpochExt {
        &self.epoch_ext
    }

    pub fn consensus(&self) -> &Consensus {
        &self.consensus
    }

    pub fn cell_set(&self) -> &HamtMap<Byte32, TransactionMeta> {
        &self.cell_set
    }

    pub fn proposals(&self) -> &ProposalView {
        &self.proposals
    }

    pub fn total_difficulty(&self) -> &U256 {
        &self.total_difficulty
    }
}

impl<'a> ChainStore<'a> for Snapshot {
    type Vector = DBPinnableSlice<'a>;

    fn get(&'a self, col: Col, key: &[u8]) -> Option<Self::Vector> {
        self.store.get(col, key)
    }

    fn get_iter<'i>(
        &'i self,
        col: Col,
        from_key: &'i [u8],
        direction: Direction,
    ) -> Box<Iterator<Item = DBIteratorItem> + 'i> {
        self.store.get_iter(col, from_key, direction)
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
        let tx_hash = out_point.tx_hash();
        let index = out_point.index().unpack();
        match self.cell_set().get(&tx_hash) {
            Some(tx_meta) => match tx_meta.is_dead(index as usize) {
                Some(false) => {
                    let mut cell_meta = self
                        .store
                        .get_cell_meta(&tx_hash, index)
                        .expect("store should be consistent with cell_set");
                    if with_data {
                        cell_meta.mem_cell_data = self.store.get_cell_data(&tx_hash, index);
                    }
                    CellStatus::live_cell(cell_meta)
                }
                Some(true) => CellStatus::Dead,
                None => CellStatus::Unknown,
            },
            None => CellStatus::Unknown,
        }
    }
}

impl HeaderChecker for Snapshot {
    fn check_valid(&self, block_hash: &Byte32) -> Result<(), UnresolvableError> {
        match self.get_block_number(block_hash) {
            Some(block_number) => {
                if self.tip_number() < block_number + self.consensus.cellbase_maturity() {
                    Err(UnresolvableError::ImmatureHeader(block_hash.clone()))
                } else {
                    Ok(())
                }
            }
            None => Err(UnresolvableError::InvalidHeader(block_hash.clone())),
        }
    }
}

impl BlockMedianTimeContext for Snapshot {
    fn median_block_count(&self) -> u64 {
        self.consensus.median_time_block_count() as u64
    }

    fn timestamp_and_parent(&self, block_hash: &Byte32) -> (u64, BlockNumber, Byte32) {
        let header = self
            .store
            .get_block_header(&block_hash)
            .expect("[ChainState] blocks used for median time exist");
        (
            header.timestamp(),
            header.number(),
            header.data().raw().parent_hash(),
        )
    }
}
