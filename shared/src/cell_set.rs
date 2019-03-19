use ckb_core::block::Block;
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

    pub fn insert(&mut self, hash: &H256, outputs_len: usize) {
        self.inner
            .insert(hash.clone(), TransactionMeta::new(outputs_len));
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
        diff.old_outputs.iter().for_each(|h| {
            self.remove(h);
        });

        diff.old_inputs.iter().for_each(|o| {
            self.mark_live(o);
        });

        diff.new_outputs.iter().for_each(|(hash, len)| {
            self.insert(hash, *len);
        });

        diff.new_inputs.iter().for_each(|o| {
            self.mark_dead(o);
        });
    }
}
