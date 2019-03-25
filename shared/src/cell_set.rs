use ckb_core::block::Block;
use ckb_core::cell::{CellProvider, CellStatus};
use ckb_core::transaction::OutPoint;
use ckb_core::transaction_meta::TransactionMeta;
use fnv::{FnvHashMap, FnvHashSet};
use numext_fixed_hash::H256;
use serde_derive::{Deserialize, Serialize};

#[derive(Default, Debug, Clone)]
pub struct CellSetDiff {
    pub old_inputs: FnvHashSet<OutPoint>,
    pub old_outputs: FnvHashSet<H256>,
    pub new_inputs: FnvHashSet<OutPoint>,
    pub new_outputs: FnvHashMap<H256, usize>,
}

impl CellSetDiff {
    pub fn push_new(&mut self, block: &Block) {
        for tx in block.commit_transactions() {
            let input_pts = tx.input_pts();
            let tx_hash = tx.hash();
            let output_len = tx.outputs().len();
            self.new_inputs.extend(input_pts);
            self.new_outputs.insert(tx_hash, output_len);
        }
    }

    pub fn push_old(&mut self, block: &Block) {
        for tx in block.commit_transactions() {
            let input_pts = tx.input_pts();
            let tx_hash = tx.hash();

            self.old_inputs.extend(input_pts);
            self.old_outputs.insert(tx_hash);
        }
    }
}

impl CellProvider for CellSetDiff {
    fn cell(&self, out_point: &OutPoint) -> CellStatus {
        if self.new_inputs.contains(out_point) {
            CellStatus::Dead
        } else {
            CellStatus::Unknown
        }
    }
}

#[derive(Default, Debug, PartialEq, Eq, Clone, Serialize, Deserialize)]
pub struct CellSet {
    pub inner: FnvHashMap<H256, TransactionMeta>,
}

impl CellSet {
    pub fn new() -> Self {
        CellSet {
            inner: FnvHashMap::default(),
        }
    }

    pub fn is_dead(&self, o: &OutPoint) -> Option<bool> {
        self.inner.get(&o.hash).map(|x| x.is_dead(o.index as usize))
    }

    pub fn get(&self, h: &H256) -> Option<&TransactionMeta> {
        self.inner.get(h)
    }

    pub fn insert(&mut self, hash: H256, outputs_len: usize) {
        self.inner.insert(hash, TransactionMeta::new(outputs_len));
    }

    pub fn remove(&mut self, hash: &H256) -> Option<TransactionMeta> {
        self.inner.remove(hash)
    }

    pub fn mark_dead(&mut self, o: &OutPoint) {
        if let Some(meta) = self.inner.get_mut(&o.hash) {
            meta.set_dead(o.index as usize);
        }
    }

    fn mark_live(&mut self, o: &OutPoint) {
        if let Some(meta) = self.inner.get_mut(&o.hash) {
            meta.unset_dead(o.index as usize);
        }
    }

    pub fn update(&mut self, diff: CellSetDiff) {
        let CellSetDiff {
            old_inputs,
            old_outputs,
            new_inputs,
            new_outputs,
        } = diff;

        old_outputs.iter().for_each(|h| {
            self.remove(h);
        });

        old_inputs.iter().for_each(|o| {
            self.mark_live(o);
        });

        new_outputs.into_iter().for_each(|(hash, len)| {
            self.insert(hash, len);
        });

        new_inputs.iter().for_each(|o| {
            self.mark_dead(o);
        });
    }
}

impl CellProvider for CellSet {
    fn cell(&self, out_point: &OutPoint) -> CellStatus {
        match self.get(&out_point.hash) {
            Some(meta) => {
                if meta.is_dead(out_point.index as usize) {
                    CellStatus::Dead
                } else {
                    CellStatus::Unknown
                }
            }
            None => CellStatus::Unknown,
        }
    }
}
