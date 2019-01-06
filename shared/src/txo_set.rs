use ckb_core::transaction::OutPoint;
use ckb_core::transaction_meta::TransactionMeta;
use fnv::FnvHashMap;
use numext_fixed_hash::H256;
use serde_derive::{Deserialize, Serialize};

#[derive(Default, Debug, Clone)]
pub struct TxoSetDiff {
    pub old_inputs: Vec<OutPoint>,
    pub old_outputs: Vec<H256>,
    pub new_inputs: Vec<OutPoint>,
    pub new_outputs: Vec<(H256, usize)>,
}

#[derive(Default, Debug, PartialEq, Eq, Clone, Serialize, Deserialize)]
pub struct TxoSet {
    pub inner: FnvHashMap<H256, TransactionMeta>,
}

impl TxoSet {
    pub fn new() -> Self {
        TxoSet {
            inner: FnvHashMap::default(),
        }
    }

    pub fn is_spent(&self, o: &OutPoint) -> Option<bool> {
        self.inner
            .get(&o.hash)
            .map(|x| x.is_spent(o.index as usize))
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

    pub fn mark_spent(&mut self, o: &OutPoint) {
        if let Some(meta) = self.inner.get_mut(&o.hash) {
            meta.set_spent(o.index as usize);
        }
    }

    pub fn mark_unspent(&mut self, o: &OutPoint) {
        if let Some(meta) = self.inner.get_mut(&o.hash) {
            meta.unset_spent(o.index as usize);
        }
    }

    pub fn roll_back(&mut self, inputs: Vec<OutPoint>, outputs: Vec<H256>) {
        for h in outputs {
            self.remove(&h);
        }

        for o in inputs {
            self.mark_unspent(&o);
        }
    }

    pub fn forward(&mut self, inputs: Vec<OutPoint>, outputs: Vec<(H256, usize)>) {
        for (hash, len) in outputs {
            self.insert(hash, len);
        }

        for o in inputs {
            self.mark_spent(&o);
        }
    }

    pub fn update(&mut self, diff: TxoSetDiff) {
        self.roll_back(diff.old_inputs, diff.old_outputs);
        self.forward(diff.new_inputs, diff.new_outputs);
    }
}
