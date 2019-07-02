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
                if let Some(meta) = new.get_mut(&cell_input.tx_hash) {
                    meta.set_dead(cell_input.index as usize);
                    continue;
                }

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

#[cfg(test)]
mod tests {
    use super::{CellSet, CellSetDiff, CellSetOpr};
    use ckb_core::block::BlockBuilder;
    use ckb_core::extras::EpochExt;
    use ckb_core::header::HeaderBuilder;
    use ckb_core::script::Script;
    use ckb_core::transaction::{
        CellInput, CellOutPoint, CellOutput, OutPoint, Transaction, TransactionBuilder,
    };
    use ckb_core::transaction_meta::TransactionMeta;
    use ckb_core::{Bytes, Capacity};
    use ckb_test_chain_utils::MockStore;
    use numext_fixed_hash::{h256, H256};

    fn build_tx(inputs: Vec<(&H256, u32)>, outputs_len: usize) -> Transaction {
        TransactionBuilder::default()
            .inputs(
                inputs.into_iter().map(|(txid, index)| {
                    CellInput::new(OutPoint::new_cell(txid.to_owned(), index), 0)
                }),
            )
            .outputs((0..outputs_len).map(|i| {
                CellOutput::new(
                    Capacity::bytes(i + 1).unwrap(),
                    Bytes::default(),
                    Script::default(),
                    None,
                )
            }))
            .build()
    }

    // Store:
    //     tx1: inputs [(0x0, 0)], outputs(2)
    //     txa: inputs [(0x1, 1)], outputs(1)
    //     tx2: inputs [(tx1, 0), (txa, 0)], outputs(1)
    // CellSet:
    //     tx1_meta(dead, live)
    //     no txa(all dead)
    //     tx2_meta(live)
    // CellSetDiff:
    //     - old: tx2
    //     - new:
    //            tx3: inputs [(tx1, 1)], outputs(1)
    //            tx4: inputs [(tx3, 1)], outputs(1)
    // The Overlay should be:
    //     tx1-meta(live, dead), txa(live) recovered, tx2-meta(_) removed, tx3-meta(dead), tx4-meta(live)
    #[test]
    fn test_new_overlay() {
        let mut store = MockStore::default();

        let tx1 = build_tx(vec![(&H256::zero(), 0)], 2);
        let tx1_hash = tx1.hash();

        let txa = build_tx(vec![(&h256!("0x1"), 0)], 1);
        let txa_hash = txa.hash();

        let tx2 = build_tx(vec![(tx1_hash, 0), (txa_hash, 0)], 1);
        let tx2_hash = tx2.hash();

        let header = HeaderBuilder::default().number(1).build();
        let block = BlockBuilder::default()
            .header(header.clone())
            .transactions(vec![tx1.clone(), txa.clone(), tx2.clone()])
            .build();

        let epoch = EpochExt::default();
        store.insert_block(&block, &epoch);

        let mut set = CellSet::new();
        let meta = set.insert_transaction(
            tx1_hash.clone(),
            header.number(),
            header.epoch(),
            false,
            tx1.outputs().len(),
        );

        let tx1_meta =
            TransactionMeta::new(header.number(), header.epoch(), tx1.outputs().len(), false);

        assert_eq!(meta, tx1_meta);
        let cell = CellOutPoint {
            tx_hash: tx1_hash.clone(),
            index: 0,
        };
        // tx2 consumed tx1-outputs-0 in block-1
        let op = set.mark_dead(&cell);

        match op {
            Some(CellSetOpr::Update(_)) => {}
            _ => panic!(),
        };

        let _ = set.insert_transaction(
            tx2_hash.clone(),
            header.number(),
            header.epoch(),
            false,
            tx2.outputs().len(),
        );

        let old_header = HeaderBuilder::default().number(2).build();
        let old_block = BlockBuilder::default()
            .header(old_header.clone())
            .transaction(tx2.clone())
            .build();

        let tx3 = build_tx(vec![(tx1_hash, 1)], 1);
        let tx3_hash = tx3.hash();

        let tx4 = build_tx(vec![(tx3_hash, 0)], 1);
        let tx4_hash = tx4.hash();

        let new_header = HeaderBuilder::default().number(2).build();
        let new_block = BlockBuilder::default()
            .header(new_header.clone())
            .transactions(vec![tx3.clone(), tx4.clone()])
            .build();

        let mut diff = CellSetDiff::default();
        diff.push_old(&old_block);
        diff.push_new(&new_block);

        let overlay = set.new_overlay(&diff, &store.0);

        let mut tx1_meta =
            TransactionMeta::new(header.number(), header.epoch(), tx1.outputs().len(), false);
        // new transaction(tx3) consumed tx1-outputs-1
        tx1_meta.set_dead(1);

        assert_eq!(overlay.get(&tx1_hash), Some(&tx1_meta));
        assert_eq!(overlay.get(&tx2_hash), None);

        // new transaction(tx4) consumed tx3-outputs
        let mut tx3_meta = TransactionMeta::new(
            new_header.number(),
            new_header.epoch(),
            tx3.outputs().len(),
            false,
        );
        tx3_meta.set_dead(0);

        assert_eq!(overlay.get(&tx3_hash), Some(&tx3_meta));

        let tx4_meta = TransactionMeta::new(
            new_header.number(),
            new_header.epoch(),
            tx4.outputs().len(),
            false,
        );

        assert_eq!(overlay.get(&tx4_hash), Some(&tx4_meta));

        let txa_meta =
            TransactionMeta::new(header.number(), header.epoch(), txa.outputs().len(), false);
        assert_eq!(overlay.get(&txa_hash), Some(&txa_meta));
    }
}
