use ckb_store::ChainStore;
use ckb_types::{
    core::{BlockView, TransactionMeta},
    packed::{Byte32, OutPoint},
    prelude::*,
    H256,
};
use std::collections::{hash_map, HashMap, HashSet};

#[derive(Default, Clone)]
pub struct CellSetDiff {
    pub old_inputs: HashSet<OutPoint>,
    pub old_outputs: HashSet<Byte32>,
    pub new_inputs: HashSet<OutPoint>,
    pub new_outputs: HashMap<Byte32, (u64, u64, Byte32, bool, usize)>,
}

impl CellSetDiff {
    pub fn push_new(&mut self, block: &BlockView) {
        for tx in block.transactions() {
            let input_iter = tx.input_pts_iter();
            let tx_hash = tx.hash();
            let output_len = tx.outputs().len();
            self.new_inputs.extend(input_iter);
            self.new_outputs.insert(
                tx_hash,
                (
                    block.number(),
                    block.epoch(),
                    block.hash(),
                    tx.is_cellbase(),
                    output_len,
                ),
            );
        }
    }

    pub fn push_old(&mut self, block: &BlockView) {
        for tx in block.transactions() {
            let input_iter = tx.input_pts_iter();
            let tx_hash = tx.hash();

            self.old_inputs.extend(input_iter);
            self.old_outputs.insert(tx_hash);
        }
    }
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct CellSetOverlay<'a> {
    origin: &'a HashMap<Byte32, TransactionMeta>,
    new: HashMap<Byte32, TransactionMeta>,
    removed: HashSet<Byte32>,
}

impl<'a> CellSetOverlay<'a> {
    pub fn get(&self, hash: &Byte32) -> Option<&TransactionMeta> {
        if self.removed.get(hash).is_some() {
            return None;
        }

        self.new.get(hash).or_else(|| self.origin.get(hash))
    }
}

#[derive(Default, Debug, PartialEq, Eq, Clone)]
pub struct CellSet {
    pub(crate) inner: HashMap<Byte32, TransactionMeta>,
}

pub(crate) enum CellSetOpr {
    Delete,
    Update(TransactionMeta),
}

impl CellSet {
    pub fn new() -> Self {
        CellSet {
            inner: HashMap::default(),
        }
    }

    pub fn new_overlay<'a, CS: ChainStore<'a>>(
        &'a self,
        diff: &CellSetDiff,
        store: &'a CS,
    ) -> CellSetOverlay<'a> {
        let mut new = HashMap::default();
        let mut removed = HashSet::default();

        for hash in &diff.old_outputs {
            if self.inner.get(&hash).is_some() {
                removed.insert(hash.clone());
            }
        }

        for (tx_hash, (number, epoch, block_hash, cellbase, len)) in diff.new_outputs.clone() {
            removed.remove(&tx_hash);
            if cellbase {
                new.insert(
                    tx_hash,
                    TransactionMeta::new_cellbase(number, epoch, block_hash.unpack(), len, false),
                );
            } else {
                new.insert(
                    tx_hash,
                    TransactionMeta::new(number, epoch, block_hash.unpack(), len, false),
                );
            }
        }

        for out_point in diff
            .old_inputs
            .iter()
            .filter(|out_point| !out_point.is_null())
        {
            if diff.old_outputs.contains(&out_point.tx_hash()) {
                continue;
            }
            if let Some(meta) = self.inner.get(&out_point.tx_hash()) {
                let meta = new
                    .entry(out_point.tx_hash())
                    .or_insert_with(|| meta.clone());
                meta.unset_dead(out_point.index().unpack());
            } else {
                // the tx is full dead, deleted from cellset, we need recover it when fork
                if let Some((tx, header)) =
                    store
                        .get_transaction(&out_point.tx_hash())
                        .and_then(|(tx, block_hash)| {
                            store
                                .get_block_header(&block_hash)
                                .map(|header| (tx, header))
                        })
                {
                    let meta = new.entry(out_point.tx_hash()).or_insert_with(|| {
                        if tx.is_cellbase() {
                            TransactionMeta::new_cellbase(
                                header.number(),
                                header.epoch(),
                                header.hash().unpack(),
                                tx.outputs().len(),
                                true,
                            )
                        } else {
                            TransactionMeta::new(
                                header.number(),
                                header.epoch(),
                                header.hash().unpack(),
                                tx.outputs().len(),
                                true,
                            )
                        }
                    });
                    meta.unset_dead(out_point.index().unpack());
                }
            }
        }

        for out_point in diff
            .new_inputs
            .iter()
            .filter(|out_point| !out_point.is_null())
        {
            if let Some(meta) = new.get_mut(&out_point.tx_hash()) {
                meta.set_dead(out_point.index().unpack());
                continue;
            }

            if let Some(meta) = self.inner.get(&out_point.tx_hash()) {
                let meta = new
                    .entry(out_point.tx_hash().clone())
                    .or_insert_with(|| meta.clone());
                meta.set_dead(out_point.index().unpack());
            }
        }

        CellSetOverlay {
            new,
            removed,
            origin: &self.inner,
        }
    }

    pub fn get(&self, h: &H256) -> Option<&TransactionMeta> {
        self.inner.get(&h.pack())
    }

    pub(crate) fn put(&mut self, tx_hash: H256, tx_meta: TransactionMeta) {
        self.inner.insert(tx_hash.pack(), tx_meta);
    }

    pub(crate) fn insert_cell(
        &mut self,
        cell: &OutPoint,
        number: u64,
        epoch: u64,
        hash: H256,
        cellbase: bool,
        outputs_len: usize,
    ) -> TransactionMeta {
        let mut meta = if cellbase {
            TransactionMeta::new_cellbase(number, epoch, hash, outputs_len, true)
        } else {
            TransactionMeta::new(number, epoch, hash, outputs_len, true)
        };
        meta.unset_dead(cell.index().unpack());
        self.inner.insert(cell.tx_hash(), meta.clone());
        meta
    }

    pub(crate) fn insert_transaction(
        &mut self,
        tx_hash: H256,
        number: u64,
        epoch: u64,
        hash: H256,
        cellbase: bool,
        outputs_len: usize,
    ) -> TransactionMeta {
        let meta = if cellbase {
            TransactionMeta::new_cellbase(number, epoch, hash, outputs_len, false)
        } else {
            TransactionMeta::new(number, epoch, hash, outputs_len, false)
        };
        self.inner.insert(tx_hash.pack(), meta.clone());
        meta
    }

    pub(crate) fn remove(&mut self, tx_hash: &H256) -> Option<TransactionMeta> {
        self.inner.remove(&tx_hash.pack())
    }

    pub(crate) fn mark_dead(&mut self, cell: &OutPoint) -> Option<CellSetOpr> {
        if let hash_map::Entry::Occupied(mut o) = self.inner.entry(cell.tx_hash()) {
            o.get_mut().set_dead(cell.index().unpack());
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
    pub(crate) fn try_mark_live(&mut self, cell: &OutPoint) -> Option<TransactionMeta> {
        if let Some(meta) = self.inner.get_mut(&cell.tx_hash()) {
            meta.unset_dead(cell.index().unpack());
            Some(meta.clone())
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ckb_test_chain_utils::MockStore;
    use ckb_types::{
        core::{
            BlockBuilder, Capacity, EpochExt, HeaderBuilder, TransactionBuilder, TransactionView,
        },
        h256,
        packed::{CellInput, CellOutputBuilder},
        H256,
    };

    fn build_tx(inputs: Vec<(&H256, u32)>, outputs_len: usize) -> TransactionView {
        TransactionBuilder::default()
            .inputs(
                inputs
                    .into_iter()
                    .map(|(txid, index)| CellInput::new(OutPoint::new(txid.to_owned(), index), 0)),
            )
            .outputs((0..outputs_len).map(|i| {
                CellOutputBuilder::default()
                    .capacity(Capacity::bytes(i + 1).unwrap().pack())
                    .build()
            }))
            .outputs_data((0..outputs_len).map(|_| Default::default()))
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
        let store = MockStore::default();

        let tx1 = build_tx(vec![(&H256::zero(), 0)], 2);
        let tx1_hash = tx1.hash().unpack();

        let txa = build_tx(vec![(&h256!("0x1"), 0)], 1);
        let txa_hash = txa.hash().unpack();

        let tx2 = build_tx(vec![(&tx1_hash, 0), (&txa_hash, 0)], 1);
        let tx2_hash: H256 = tx2.hash().unpack();

        let block = BlockBuilder::default()
            .header(HeaderBuilder::default().number(1.pack()).build())
            .transactions(vec![tx1.clone(), txa.clone(), tx2.clone()])
            .build();
        let header = block.header();

        let epoch = EpochExt::default();
        store.insert_block(&block, &epoch);

        let mut set = CellSet::new();
        let meta = set.insert_transaction(
            tx1_hash.clone(),
            header.number(),
            header.epoch(),
            header.hash().to_owned().unpack(),
            false,
            tx1.outputs().len(),
        );

        let tx1_meta = TransactionMeta::new(
            header.number(),
            header.epoch(),
            header.hash().to_owned().unpack(),
            tx1.outputs().len(),
            false,
        );

        assert_eq!(meta, tx1_meta);
        let cell = OutPoint::new_builder()
            .tx_hash(tx1_hash.clone().pack())
            .build();
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
            header.hash().to_owned().unpack(),
            false,
            tx2.outputs().len(),
        );

        let old_block = BlockBuilder::default()
            .header(HeaderBuilder::default().number(2.pack()).build())
            .transaction(tx2.clone())
            .build();

        let tx3 = build_tx(vec![(&tx1_hash, 1)], 1);
        let tx3_hash = tx3.hash().unpack();

        let tx4 = build_tx(vec![(&tx3_hash, 0)], 1);
        let tx4_hash: H256 = tx4.hash().unpack();

        let new_block = BlockBuilder::default()
            .header(HeaderBuilder::default().number(2.pack()).build())
            .transactions(vec![tx3.clone(), tx4.clone()])
            .build();
        let new_header = new_block.header();

        let mut diff = CellSetDiff::default();
        diff.push_old(&old_block);
        diff.push_new(&new_block);

        let overlay = set.new_overlay(&diff, store.store());

        let mut tx1_meta = TransactionMeta::new(
            header.number(),
            header.epoch(),
            header.hash().to_owned().unpack(),
            tx1.outputs().len(),
            false,
        );
        // new transaction(tx3) consumed tx1-outputs-1
        tx1_meta.set_dead(1);

        assert_eq!(overlay.get(&tx1_hash.pack()), Some(&tx1_meta));
        assert_eq!(overlay.get(&tx2_hash.pack()), None);

        // new transaction(tx4) consumed tx3-outputs
        let mut tx3_meta = TransactionMeta::new(
            new_header.number(),
            new_header.epoch(),
            new_header.hash().to_owned().unpack(),
            tx3.outputs().len(),
            false,
        );
        tx3_meta.set_dead(0);

        assert_eq!(overlay.get(&tx3_hash.pack()), Some(&tx3_meta));

        let tx4_meta = TransactionMeta::new(
            new_header.number(),
            new_header.epoch(),
            new_header.hash().to_owned().unpack(),
            tx4.outputs().len(),
            false,
        );

        assert_eq!(overlay.get(&tx4_hash.pack()), Some(&tx4_meta));

        let txa_meta = TransactionMeta::new(
            header.number(),
            header.epoch(),
            header.hash().to_owned().unpack(),
            txa.outputs().len(),
            false,
        );
        assert_eq!(overlay.get(&txa_hash.pack()), Some(&txa_meta));
    }
}
