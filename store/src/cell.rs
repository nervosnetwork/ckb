use crate::{ChainStore, StoreTransaction};
use ckb_error::Error;
use ckb_types::{core::BlockView, packed, prelude::*};
use std::collections::HashMap;

/**
 * Live cell entry.
 *
 *  table CellEntry {
 *      output:                CellOutput,
 *      block_hash:            Byte32,
 *      block_number:          Uint64,
 *      block_epoch:           Uint64,
 *      index:                 Uint32,
 *      data_size:             Uint64,
 *  }
 *
 *
 *  table CellDataEntry {
 *      output_data:           Bytes,
 *      output_data_hash:      Byte32,
 *  }
 */

// Apply the effects of this block on the live cell set.
pub fn attach_block_cell(txn: &StoreTransaction, block: &BlockView) -> Result<(), Error> {
    let transactions = block.transactions();

    // add new live cells
    let new_cells = transactions
        .iter()
        .enumerate()
        .map(move |(tx_index, tx)| {
            let tx_hash = tx.hash();
            let block_hash = block.header().hash();
            let block_number = block.header().number();
            let block_epoch = block.header().epoch();

            tx.outputs_with_data_iter()
                .enumerate()
                .map(move |(index, (cell_output, data))| {
                    let out_point = packed::OutPoint::new_builder()
                        .tx_hash(tx_hash.clone())
                        .index(index.pack())
                        .build();

                    let entry = packed::CellEntryBuilder::default()
                        .output(cell_output)
                        .block_hash(block_hash.clone())
                        .block_number(block_number.pack())
                        .block_epoch(block_epoch.pack())
                        .index(tx_index.pack())
                        .data_size((data.len() as u64).pack())
                        .build();

                    let data_entry = if !data.is_empty() {
                        let data_hash = packed::CellOutput::calc_data_hash(&data);
                        Some(
                            packed::CellDataEntryBuilder::default()
                                .output_data(data.pack())
                                .output_data_hash(data_hash)
                                .build(),
                        )
                    } else {
                        None
                    };

                    (out_point, entry, data_entry)
                })
        })
        .flatten();
    txn.insert_cells(new_cells)?;

    // mark inputs dead
    // skip cellbase
    let deads = transactions
        .iter()
        .skip(1)
        .map(|tx| tx.input_pts_iter())
        .flatten();
    txn.delete_cells(deads)?;

    Ok(())
}

/// Undoes the effects of this block on the live cell set.
pub fn detach_block_cell(txn: &StoreTransaction, block: &BlockView) -> Result<(), Error> {
    let transactions = block.transactions();
    let mut input_pts = HashMap::with_capacity(transactions.len());

    for tx in transactions.iter().skip(1) {
        for pts in tx.input_pts_iter() {
            let tx_hash = pts.tx_hash();
            let index: usize = pts.index().unpack();
            let indexes = input_pts.entry(tx_hash).or_insert_with(Vec::new);
            indexes.push(index);
        }
    }

    // restore inputs
    // skip cellbase
    let undo_deads = input_pts
        .iter()
        .filter_map(|(tx_hash, indexes)| {
            txn.get_transaction_with_info(&tx_hash)
                .map(move |(tx, info)| {
                    let block_hash = info.block_hash;
                    let block_number = info.block_number;
                    let block_epoch = info.block_epoch;
                    let tx_index = info.index;

                    indexes.iter().filter_map(move |index| {
                        tx.output_with_data(*index).map(|(cell_output, data)| {
                            let out_point = packed::OutPoint::new_builder()
                                .tx_hash(tx_hash.clone())
                                .index(index.pack())
                                .build();

                            let entry = packed::CellEntryBuilder::default()
                                .output(cell_output)
                                .block_hash(block_hash.clone())
                                .block_number(block_number.pack())
                                .block_epoch(block_epoch.pack())
                                .index(tx_index.pack())
                                .data_size((data.len() as u64).pack())
                                .build();

                            let data_entry = if !data.is_empty() {
                                let data_hash = packed::CellOutput::calc_data_hash(&data);
                                Some(
                                    packed::CellDataEntryBuilder::default()
                                        .output_data(data.pack())
                                        .output_data_hash(data_hash)
                                        .build(),
                                )
                            } else {
                                None
                            };

                            (out_point, entry, data_entry)
                        })
                    })
                })
        })
        .flatten();
    txn.insert_cells(undo_deads)?;

    // undo live cells
    let undo_cells = transactions.iter().map(|tx| tx.output_pts_iter()).flatten();
    txn.delete_cells(undo_cells)?;

    Ok(())
}
