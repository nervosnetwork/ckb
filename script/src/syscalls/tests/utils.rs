use ckb_traits::{CellDataProvider, HeaderProvider};
use ckb_types::{
    bytes::Bytes,
    core::{cell::CellMeta, Capacity, HeaderView},
    packed::{Byte32, CellOutput, OutPoint},
    prelude::*,
};
use std::collections::HashMap;

#[derive(Default, PartialEq, Eq, Clone)]
pub(crate) struct MockDataLoader {
    pub(crate) headers: HashMap<Byte32, HeaderView>,
}

impl CellDataProvider for MockDataLoader {
    fn get_cell_data(&self, _out_point: &OutPoint) -> Option<Bytes> {
        None
    }

    fn get_cell_data_hash(&self, _out_point: &OutPoint) -> Option<Byte32> {
        None
    }
}

impl HeaderProvider for MockDataLoader {
    fn get_header(&self, block_hash: &Byte32) -> Option<HeaderView> {
        self.headers.get(block_hash).cloned()
    }
}

pub(crate) fn new_mock_data_loader() -> MockDataLoader {
    MockDataLoader::default()
}

pub(crate) fn build_cell_meta(capacity_bytes: usize, data: Bytes) -> CellMeta {
    let capacity = Capacity::bytes(capacity_bytes).expect("capacity bytes overflow");
    let builder = CellOutput::new_builder().capacity(capacity.pack());
    let data_hash = CellOutput::calc_data_hash(&data);
    CellMeta {
        out_point: OutPoint::default(),
        transaction_info: None,
        cell_output: builder.build(),
        data_bytes: data.len() as u64,
        mem_cell_data: Some(data),
        mem_cell_data_hash: Some(data_hash),
    }
}
