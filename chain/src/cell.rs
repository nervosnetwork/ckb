use ckb_error::Error;
use ckb_store::{ChainStore, StoreTransaction};
use ckb_types::{
    core::{BlockView, TransactionMeta},
    prelude::*,
};

pub fn attach_block_cell(txn: &StoreTransaction, block: &BlockView) -> Result<(), Error> {
    for tx in block.transactions() {
        for cell in tx.input_pts_iter() {
            let cell_tx_hash = cell.tx_hash();
            if let Some(mut meta) = txn.get_tx_meta(&cell_tx_hash) {
                meta.set_dead(cell.index().unpack());
                if meta.all_dead() {
                    txn.delete_cell_set(&cell_tx_hash)?;
                } else {
                    txn.update_cell_set(&cell_tx_hash, &meta.pack())?;
                }
            }
        }
        let tx_hash = tx.hash();
        let outputs_len = tx.outputs().len();
        let meta = if tx.is_cellbase() {
            TransactionMeta::new_cellbase(
                block.number(),
                block.epoch().number(),
                block.hash(),
                outputs_len,
                false,
            )
        } else {
            TransactionMeta::new(
                block.number(),
                block.epoch().number(),
                block.hash(),
                outputs_len,
                false,
            )
        };
        txn.update_cell_set(&tx_hash, &meta.pack())?;
    }
    Ok(())
}

pub fn detach_block_cell(txn: &StoreTransaction, block: &BlockView) -> Result<(), Error> {
    for tx in block.transactions().iter().rev() {
        txn.delete_cell_set(&tx.hash())?;

        for cell in tx.input_pts_iter() {
            let cell_tx_hash = cell.tx_hash();
            let index: usize = cell.index().unpack();

            if let Some(mut tx_meta) = txn.get_tx_meta(&cell_tx_hash) {
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
                            header.epoch().number(),
                            header.hash(),
                            tx.outputs().len(),
                            true, // init with all dead
                        )
                    } else {
                        TransactionMeta::new(
                            header.number(),
                            header.epoch().number(),
                            header.hash(),
                            tx.outputs().len(),
                            true, // init with all dead
                        )
                    };
                    meta.unset_dead(index); // recover
                    txn.update_cell_set(&cell_tx_hash, &meta.pack())?;
                }
            }
        }
    }
    Ok(())
}
