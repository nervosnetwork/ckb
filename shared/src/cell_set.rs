use ckb_core::transaction::OutPoint;
use ckb_core::transaction_meta::TransactionMeta;
use fnv::FnvHashMap;
use numext_fixed_hash::H256;
use serde_derive::{Deserialize, Serialize};

#[derive(Default, Debug, Clone)]
pub struct CellSetDiff {
    pub old_inputs: Vec<OutPoint>,
    pub old_outputs: Vec<H256>,
    pub new_inputs: Vec<OutPoint>,
    pub new_outputs: Vec<(H256, usize)>,
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

    fn rollback(&mut self, inputs: Vec<OutPoint>, outputs: Vec<H256>) {
        for h in outputs {
            self.remove(&h);
        }

        for o in inputs {
            self.mark_live(&o);
        }
    }

    fn forward(&mut self, inputs: Vec<OutPoint>, outputs: Vec<(H256, usize)>) {
        for (hash, len) in outputs {
            self.insert(hash, len);
        }

        for o in inputs {
            self.mark_dead(&o);
        }
    }

    pub fn update(&mut self, diff: CellSetDiff) {
        self.rollback(diff.old_inputs, diff.old_outputs);
        self.forward(diff.new_inputs, diff.new_outputs);
    }
}
