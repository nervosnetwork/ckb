use crate::{ChainStore, StoreTransaction};
use ckb_error::Error;
use ckb_types::{
    bytes::Bytes,
    core::{BlockView, TransactionInfo},
    packed,
    prelude::*,
};
use std::collections::HashMap;

// Attaching an output and restoring it after a detach must write the same
// origin, payload and data hash for its out point.
fn live_cell(
    tx_hash: &packed::Byte32,
    index: usize,
    (output, data): (packed::CellOutput, Bytes),
    origin: &TransactionInfo,
) -> (
    packed::OutPoint,
    packed::CellEntry,
    Option<packed::CellDataEntry>,
) {
    let point = packed::OutPoint::new_builder()
        .tx_hash(tx_hash.clone())
        .index(index)
        .build();
    let cell = packed::CellEntry::new_builder()
        .output(output)
        .block_hash(origin.block_hash.clone())
        .block_number(origin.block_number)
        .block_epoch(origin.block_epoch)
        .index(origin.index)
        .data_size(data.len() as u64)
        .build();
    let data = (!data.is_empty()).then(|| {
        let hash = packed::CellOutput::calc_data_hash(&data);
        packed::CellDataEntry::new_builder()
            .output_data(data)
            .output_data_hash(hash)
            .build()
    });
    (point, cell, data)
}

/// Apply the effects of this block on the live cell set.
pub fn attach_block_cell(txn: &StoreTransaction, block: &BlockView) -> Result<(), Error> {
    let transactions = block.transactions();
    let block_hash = block.hash();
    let block_number = block.number();
    let block_epoch = block.epoch();

    // Keep original transaction indices while writing outputs in database-key
    // order. Stable sorting preserves the final origin for repeated hashes.
    let mut output_order: Vec<_> = transactions.iter().enumerate().collect();
    output_order.sort_by(|(_, left), (_, right)| left.hash_ref().cmp(right.hash_ref()));
    let new_cells = output_order.into_iter().flat_map(|(tx_index, tx)| {
        let origin = TransactionInfo::new(block_number, block_epoch, block_hash.clone(), tx_index);
        tx.outputs_with_data_iter()
            .enumerate()
            .map(move |(index, output)| live_cell(tx.hash_ref(), index, output, &origin))
    });
    txn.insert_cells(new_cells)?;

    // mark inputs dead
    // skip cellbase
    let mut deads: Vec<_> = transactions
        .iter()
        .skip(1)
        .flat_map(|tx| tx.input_pts_iter())
        .collect();
    // Cell keys sort by hash and then by the numeric output index.
    deads.sort_unstable();
    txn.delete_cells(deads.into_iter())?;

    Ok(())
}

/// Undoes the effects of this block on the live cell set.
pub fn detach_block_cell(txn: &StoreTransaction, block: &BlockView) -> Result<(), Error> {
    let transactions = block.transactions();
    let mut input_pts = HashMap::with_capacity(transactions.len());

    for tx in transactions.iter().skip(1) {
        for pts in tx.input_pts_iter() {
            let tx_hash = pts.tx_hash();
            let index: usize = pts.index().into();
            let indexes = input_pts.entry(tx_hash).or_insert_with(Vec::new);
            indexes.push(index);
        }
    }

    // restore inputs
    // skip cellbase
    let undo_deads = input_pts
        .into_iter()
        .filter_map(|(tx_hash, indexes)| {
            let (tx, origin) = txn.get_transaction_with_info(&tx_hash)?;
            Some(indexes.into_iter().filter_map(move |index| {
                let output = tx.output_with_data(index)?;
                Some(live_cell(&tx_hash, index, output, &origin))
            }))
        })
        .flatten();
    txn.insert_cells(undo_deads)?;

    // undo live cells
    let undo_cells = transactions.iter().flat_map(|tx| tx.output_pts_iter());
    txn.delete_cells(undo_cells)?;

    Ok(())
}
