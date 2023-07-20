use std::io::{Cursor, Write};

use ckb_hash::blake2b_256;
use golomb_coded_set::{GCSFilterWriter, SipHasher24Builder, M, P};

use crate::{core::TransactionView, packed, prelude::*};

/// Provides data for building block filter data.
pub trait FilterDataProvider {
    /// Finds the cell through its out point.
    fn cell(&self, out_point: &packed::OutPoint) -> Option<packed::CellOutput>;
}

/// Builds filter data for transactions.
pub fn build_filter_data<P: FilterDataProvider>(
    provider: P,
    transactions: &[TransactionView],
) -> (Vec<u8>, Vec<packed::OutPoint>) {
    let mut filter_writer = Cursor::new(Vec::new());
    let mut filter = build_gcs_filter(&mut filter_writer);
    let mut missing_out_points = Vec::new();
    for tx in transactions {
        if !tx.is_cellbase() {
            for out_point in tx.input_pts_iter() {
                if let Some(input_cell) = provider.cell(&out_point) {
                    filter.add_element(input_cell.calc_lock_hash().as_slice());
                    if let Some(type_script) = input_cell.type_().to_opt() {
                        filter.add_element(type_script.calc_script_hash().as_slice());
                    }
                } else {
                    missing_out_points.push(out_point);
                }
            }
        }
        for output_cell in tx.outputs() {
            filter.add_element(output_cell.calc_lock_hash().as_slice());
            if let Some(type_script) = output_cell.type_().to_opt() {
                filter.add_element(type_script.calc_script_hash().as_slice());
            }
        }
    }
    filter
        .finish()
        .expect("flush to memory writer should be OK");
    let filter_data = filter_writer.into_inner();
    (filter_data, missing_out_points)
}

/// Calculates a block filter hash.
pub fn calc_filter_hash(
    parent_block_filter_hash: &packed::Byte32,
    filter_data: &packed::Bytes,
) -> [u8; 32] {
    blake2b_256(
        [
            parent_block_filter_hash.as_slice(),
            filter_data.calc_raw_data_hash().as_slice(),
        ]
        .concat(),
    )
}

fn build_gcs_filter(out: &mut dyn Write) -> GCSFilterWriter<SipHasher24Builder> {
    GCSFilterWriter::new(out, SipHasher24Builder::new(0, 0), M, P)
}
