use ckb_core::block::Block;
use ckb_core::transaction::OutPoint;
use ckb_core::transaction_meta::TransactionMeta;
use fnv::{FnvHashMap, FnvHashSet};
use numext_fixed_hash::H256;
use serde_derive::{Deserialize, Serialize};

#[derive(Default, Clone, Deserialize, Serialize)]
pub struct CellSetDiff {
    pub old_inputs: FnvHashSet<OutPoint>,
    pub old_outputs: FnvHashSet<H256>,
    pub new_inputs: FnvHashSet<OutPoint>,
    pub new_outputs: FnvHashMap<H256, (u64, bool, usize)>,
}

impl CellSetDiff {
    pub fn push_new(&mut self, block: &Block) {
        for tx in block.commit_transactions() {
            let input_pts = tx.input_pts();
            let tx_hash = tx.hash();
            let output_len = tx.outputs().len();
            self.new_inputs.extend(input_pts);
            self.new_outputs.insert(
                tx_hash,
                (block.header().number(), tx.is_cellbase(), output_len),
            );
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

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct CellSetOverlay<'a> {
    origin: &'a FnvHashMap<H256, TransactionMeta>,
    new: FnvHashMap<H256, TransactionMeta>,
    removed: FnvHashSet<H256>,
}

impl<'a> CellSetOverlay<'a> {
    pub fn get(&self, hash: &H256) -> Option<&TransactionMeta> {
        if self.removed.get(hash).is_some() {
            return None;
        }

        self.new.get(hash).or_else(|| self.origin.get(hash))
    }
}

#[derive(Default, Debug, PartialEq, Eq, Clone, Serialize, Deserialize)]
pub struct CellSet {
    pub(crate) inner: FnvHashMap<H256, TransactionMeta>,
}

impl CellSet {
    pub fn new() -> Self {
        CellSet {
            inner: FnvHashMap::default(),
        }
    }

    pub fn new_overlay<'a>(&'a self, diff: &CellSetDiff) -> CellSetOverlay<'a> {
        let mut new = FnvHashMap::default();
        let mut removed = FnvHashSet::default();

        for hash in &diff.old_outputs {
            if self.inner.get(&hash).is_some() {
                removed.insert(hash.clone());
            }
        }

        for (hash, (number, cellbase, len)) in diff.new_outputs.clone() {
            removed.remove(&hash);
            if cellbase {
                new.insert(hash, TransactionMeta::new_cellbase(number, len));
            } else {
                new.insert(hash, TransactionMeta::new(number, len));
            }
        }

        for old_input in &diff.old_inputs {
            if let Some(meta) = self.inner.get(&old_input.hash) {
                let meta = new
                    .entry(old_input.hash.clone())
                    .or_insert_with(|| meta.clone());
                meta.unset_dead(old_input.index as usize);
            }
        }

        for new_input in &diff.new_inputs {
            if let Some(meta) = self.inner.get(&new_input.hash) {
                let meta = new
                    .entry(new_input.hash.clone())
                    .or_insert_with(|| meta.clone());
                meta.set_dead(new_input.index as usize);
            }
        }

        CellSetOverlay {
            new,
            removed,
            origin: &self.inner,
        }
    }

    pub fn is_dead(&self, o: &OutPoint) -> Option<bool> {
        self.inner.get(&o.hash).map(|x| x.is_dead(o.index as usize))
    }

    pub fn get(&self, h: &H256) -> Option<&TransactionMeta> {
        self.inner.get(h)
    }

    pub fn insert(&mut self, hash: H256, number: u64, cellbase: bool, outputs_len: usize) {
        if cellbase {
            self.inner
                .insert(hash, TransactionMeta::new_cellbase(number, outputs_len));
        } else {
            self.inner
                .insert(hash, TransactionMeta::new(number, outputs_len));
        }
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

        new_outputs
            .into_iter()
            .for_each(|(hash, (number, cellbase, len))| {
                self.insert(hash, number, cellbase, len);
            });

        new_inputs.iter().for_each(|o| {
            self.mark_dead(o);
        });
    }
}
