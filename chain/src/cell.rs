use ckb_error::Error;
use ckb_store::{ChainStore, StoreTransaction};
use ckb_types::{
    core::{BlockView, TransactionMeta},
    packed::Byte32,
    prelude::*,
};
use im::hashmap as hamt;
use im::hashmap::HashMap as HamtMap;

pub fn attach_block_cell(
    txn: &StoreTransaction,
    block: &BlockView,
    cell_set: &mut HamtMap<Byte32, TransactionMeta>,
) -> Result<(), Error> {
    for tx in block.transactions() {
        for cell in tx.input_pts_iter() {
            if let hamt::Entry::Occupied(mut o) = cell_set.entry(cell.tx_hash().clone()) {
                o.get_mut().set_dead(cell.index().unpack());
                if o.get().all_dead() {
                    txn.delete_cell_set(&cell.tx_hash())?;
                    o.remove_entry();
                } else {
                    txn.update_cell_set(&cell.tx_hash(), &o.get().pack())?;
                }
            }
        }
        let tx_hash = tx.hash();
        let outputs_len = tx.outputs().len();
        let meta = if tx.is_cellbase() {
            TransactionMeta::new_cellbase(
                block.number(),
                block.epoch(),
                block.hash(),
                outputs_len,
                false,
            )
        } else {
            TransactionMeta::new(
                block.number(),
                block.epoch(),
                block.hash(),
                outputs_len,
                false,
            )
        };
        txn.update_cell_set(&tx_hash, &meta.pack())?;
        cell_set.insert(tx_hash.to_owned(), meta);
    }
    Ok(())
}

pub fn detach_block_cell(
    txn: &StoreTransaction,
    block: &BlockView,
    cell_set: &mut HamtMap<Byte32, TransactionMeta>,
) -> Result<(), Error> {
    for tx in block.transactions().iter().rev() {
        txn.delete_cell_set(&tx.hash())?;
        cell_set.remove(&tx.hash());

        for cell in tx.input_pts_iter() {
            let cell_tx_hash = cell.tx_hash();
            let index: usize = cell.index().unpack();
            if let Some(tx_meta) = cell_set.get_mut(&cell_tx_hash) {
                tx_meta.unset_dead(index);
                txn.update_cell_set(&cell_tx_hash, &tx_meta.pack())?;
            } else {
                // the tx is full dead, deleted from cellset, we need recover it when fork
                if let Some((tx, header)) =
                    txn.get_transaction(&cell_tx_hash)
                        .and_then(|(tx, block_hash)| {
                            txn.get_block_header(&block_hash).map(|header| (tx, header))
                        })
                {
                    let mut meta = if tx.is_cellbase() {
                        TransactionMeta::new_cellbase(
                            header.number(),
                            header.epoch(),
                            header.hash(),
                            tx.outputs().len(),
                            true, // init with all dead
                        )
                    } else {
                        TransactionMeta::new(
                            header.number(),
                            header.epoch(),
                            header.hash(),
                            tx.outputs().len(),
                            true, // init with all dead
                        )
                    };
                    meta.unset_dead(index); // recover
                    txn.update_cell_set(&cell_tx_hash, &meta.pack())?;
                    cell_set.insert(cell_tx_hash, meta);
                }
            }
        }
    }
    Ok(())
}
