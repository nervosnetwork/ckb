use crate::Node;
use ckb_types::core::cell::{CellMeta, CellMetaBuilder};
use ckb_types::core::{BlockView, TransactionInfo};
use ckb_types::packed::{Byte32, CellInput, CellOutput, OutPoint};
use ckb_types::prelude::*;
use std::collections::HashMap;
use std::ops::Range;

pub fn gen_spendable(node: &Node, count: usize) -> Vec<CellMeta> {
    // TODO optimize based on `out_of_boostrap_period`
    loop {
        let mut spendable = get_spendable(node);
        if count > spendable.len() {
            let gap = count - spendable.len();
            node.generate_blocks(gap);
        } else {
            break spendable.split_off(spendable.len() - count);
        }
    }
}

pub fn get_spendable(node: &Node) -> Vec<CellMeta> {
    let numbers = 1..node.get_tip_block_number() + 1;
    get_spendable_by_numbers(node, numbers)
}

pub fn get_spendable_by_numbers(node: &Node, numbers: Range<u64>) -> Vec<CellMeta> {
    let block_hashes = numbers.map(|number| node.rpc_client().get_block_hash(number).unwrap());
    get_spendable_by_hashes(node, block_hashes)
}

pub fn get_spendable_by_hashes<BlockHashes>(node: &Node, block_hashes: BlockHashes) -> Vec<CellMeta>
where
    BlockHashes: IntoIterator<Item = Byte32>,
{
    let blocks = block_hashes
        .into_iter()
        .map(|block_hash| node.get_block(block_hash));
    get_spendable_by_blocks(node, blocks)
}

pub fn get_spendable_by_blocks<I>(node: &Node, blocks: I) -> Vec<CellMeta>
where
    I: IntoIterator<Item = BlockView>,
{
    let mut spendable = HashMap::new();
    for block in blocks {
        for (txindex, transaction) in block.transactions().into_iter().enumerate() {
            let txhash = transaction.hash();
            let txinfo = TransactionInfo::new(block.number(), block.epoch(), block.hash(), txindex);
            for input in transaction.input_pts_iter() {
                spendable.remove(&input);
            }
            for (oindex, (output, output_data)) in transaction.outputs_with_data_iter().enumerate()
            {
                let out_point = OutPoint::new(txhash.clone(), oindex as u32);
                let cell_meta = CellMetaBuilder::from_cell_output(output, output_data)
                    .out_point(out_point.clone())
                    .transaction_info(txinfo.clone())
                    .build();
                spendable.insert(out_point, cell_meta);
            }
        }
    }

    // Prune un-matured cellbase cells
    let tip_epoch = node.get_tip_block().epoch();
    let cellbase_maturity = node.consensus().cellbase_maturity();
    spendable.retain(|_, cell_meta| {
        if cell_meta.is_cellbase() {
            let epoch = cell_meta
                .transaction_info
                .as_ref()
                .map(|txinfo| txinfo.block_epoch)
                .unwrap();
            epoch.to_rational() + cellbase_maturity.to_rational() <= tip_epoch.to_rational()
        } else {
            true
        }
    });

    // Prune dead cells
    spendable.retain(|out_point, _| {
        node.rpc_client()
            .get_live_cell(out_point.clone().into(), false)
            .cell
            .is_some()
    });

    spendable.values().cloned().collect()
}

pub fn as_inputs(cells: &[CellMeta]) -> Vec<CellInput> {
    cells.iter().map(as_input).collect()
}

pub fn as_outputs(cells: &[CellMeta]) -> Vec<CellOutput> {
    cells.iter().map(as_output).collect()
}

pub fn as_input(cell: &CellMeta) -> CellInput {
    let block_number = cell
        .transaction_info
        .as_ref()
        .map(|txinfo| txinfo.block_number)
        .unwrap();
    let out_point = cell.out_point.clone();
    CellInput::new(out_point, block_number)
}

pub fn as_output(cell: &CellMeta) -> CellOutput {
    CellOutput::new_builder()
        .lock(cell.cell_output.lock())
        .type_(cell.cell_output.type_())
        .capacity(cell.capacity().pack())
        .build()
}
