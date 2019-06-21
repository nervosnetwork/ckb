use ckb_core::block::Block;
use ckb_core::transaction::{CellOutPoint, OutPoint};
use ckb_core::transaction_meta::TransactionMeta;
use ckb_store::ChainStore;
use ckb_util::{FnvHashMap, FnvHashSet};
use numext_fixed_hash::H256;
use serde_derive::{Deserialize, Serialize};
use std::collections::hash_map;
use std::sync::Arc;

#[derive(Default, Clone, Deserialize, Serialize)]
pub struct CellSetDiff {
    pub old_inputs: FnvHashSet<OutPoint>,
    pub old_outputs: FnvHashSet<H256>,
    pub new_inputs: FnvHashSet<OutPoint>,
    pub new_outputs: FnvHashMap<H256, (u64, u64, bool, usize)>,
}

impl CellSetDiff {
    pub fn push_new(&mut self, block: &Block) {
        for tx in block.transactions() {
            let input_iter = tx.input_pts_iter();
            let tx_hash = tx.hash();
            let output_len = tx.outputs().len();
            self.new_inputs.extend(input_iter.cloned());
            self.new_outputs.insert(
                tx_hash.to_owned(),
                (
                    block.header().number(),
                    block.header().epoch(),
                    tx.is_cellbase(),
                    output_len,
                ),
            );
        }
    }

    pub fn push_old(&mut self, block: &Block) {
        for tx in block.transactions() {
            let input_iter = tx.input_pts_iter();
            let tx_hash = tx.hash();

            self.old_inputs.extend(input_iter.cloned());
            self.old_outputs.insert(tx_hash.to_owned());
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

pub(crate) enum CellSetOpr {
    Delete,
    Update(TransactionMeta),
}

impl CellSet {
    pub fn new() -> Self {
        CellSet {
            inner: FnvHashMap::default(),
        }
    }

    pub fn new_overlay<'a, CS: ChainStore>(
        &'a self,
        diff: &CellSetDiff,
        store: &Arc<CS>,
    ) -> CellSetOverlay<'a> {
        let mut new = FnvHashMap::default();
        let mut removed = FnvHashSet::default();

        for hash in &diff.old_outputs {
            if self.inner.get(&hash).is_some() {
                removed.insert(hash.clone());
            }
        }

        for (hash, (number, epoch, cellbase, len)) in diff.new_outputs.clone() {
            removed.remove(&hash);
            if cellbase {
                new.insert(
                    hash,
                    TransactionMeta::new_cellbase(number, epoch, len, false),
                );
            } else {
                new.insert(hash, TransactionMeta::new(number, epoch, len, false));
            }
        }

        for old_input in &diff.old_inputs {
            if let Some(cell_input) = &old_input.cell {
                if diff.old_outputs.contains(&cell_input.tx_hash) {
                    continue;
                }
                if let Some(meta) = self.inner.get(&cell_input.tx_hash) {
                    let meta = new
                        .entry(cell_input.tx_hash.clone())
                        .or_insert_with(|| meta.clone());
                    meta.unset_dead(cell_input.index as usize);
                } else {
                    // the tx is full dead, deleted from cellset, we need recover it when fork
                    if let Some((tx, header)) =
                        store
                            .get_transaction(&cell_input.tx_hash)
                            .and_then(|(tx, block_hash)| {
                                store
                                    .get_block_header(&block_hash)
                                    .map(|header| (tx, header))
                            })
                    {
                        let meta = new.entry(cell_input.tx_hash.clone()).or_insert_with(|| {
                            if tx.is_cellbase() {
                                TransactionMeta::new_cellbase(
                                    header.number(),
                                    header.epoch(),
                                    tx.outputs().len(),
                                    true,
                                )
                            } else {
                                TransactionMeta::new(
                                    header.number(),
                                    header.epoch(),
                                    tx.outputs().len(),
                                    true,
                                )
                            }
                        });
                        meta.unset_dead(cell_input.index as usize);
                    }
                }
            }
        }

        for new_input in &diff.new_inputs {
            if let Some(cell_input) = &new_input.cell {
                if let Some(meta) = self.inner.get(&cell_input.tx_hash) {
                    let meta = new
                        .entry(cell_input.tx_hash.clone())
                        .or_insert_with(|| meta.clone());
                    meta.set_dead(cell_input.index as usize);
                }
            }
        }

        CellSetOverlay {
            new,
            removed,
            origin: &self.inner,
        }
    }

    pub fn get(&self, h: &H256) -> Option<&TransactionMeta> {
        self.inner.get(h)
    }

    pub(crate) fn put(&mut self, tx_hash: H256, tx_meta: TransactionMeta) {
        self.inner.insert(tx_hash, tx_meta);
    }

    pub(crate) fn insert_cell(
        &mut self,
        cell: &CellOutPoint,
        number: u64,
        epoch: u64,
        cellbase: bool,
        outputs_len: usize,
    ) -> TransactionMeta {
        let mut meta = if cellbase {
            TransactionMeta::new_cellbase(number, epoch, outputs_len, true)
        } else {
            TransactionMeta::new(number, epoch, outputs_len, true)
        };
        meta.unset_dead(cell.index as usize);
        self.inner.insert(cell.tx_hash.clone(), meta.clone());
        meta
    }

    pub(crate) fn insert_transaction(
        &mut self,
        tx_hash: H256,
        number: u64,
        epoch: u64,
        cellbase: bool,
        outputs_len: usize,
    ) -> TransactionMeta {
        let meta = if cellbase {
            TransactionMeta::new_cellbase(number, epoch, outputs_len, false)
        } else {
            TransactionMeta::new(number, epoch, outputs_len, false)
        };
        self.inner.insert(tx_hash, meta.clone());
        meta
    }

    pub(crate) fn remove(&mut self, tx_hash: &H256) -> Option<TransactionMeta> {
        self.inner.remove(tx_hash)
    }

    pub(crate) fn mark_dead(&mut self, cell: &CellOutPoint) -> Option<CellSetOpr> {
        if let hash_map::Entry::Occupied(mut o) = self.inner.entry(cell.tx_hash.clone()) {
            o.get_mut().set_dead(cell.index as usize);
            if o.get().all_dead() {
                o.remove_entry();
                Some(CellSetOpr::Delete)
            } else {
                Some(CellSetOpr::Update(o.get().clone()))
            }
        } else {
            None
        }
    }

    // if we aleady removed the cell, `mark` will return None, else return the meta
    pub(crate) fn try_mark_live(&mut self, cell: &CellOutPoint) -> Option<TransactionMeta> {
        if let Some(meta) = self.inner.get_mut(&cell.tx_hash) {
            meta.unset_dead(cell.index as usize);
            Some(meta.clone())
        } else {
            None
        }
    }
}
