use crate::{
    types::{
        DataPieceId, ScriptGroup, ScriptGroupType, ScriptVersion, SgData, SgInfo, TxData, TxInfo,
    },
    verify_env::TxVerifyEnv,
};
use ckb_chain_spec::consensus::ConsensusBuilder;
use ckb_traits::{CellDataProvider, ExtensionProvider, HeaderProvider};
use ckb_types::{
    bytes::Bytes,
    core::{
        Capacity, HeaderBuilder, HeaderView,
        cell::{CellMeta, ResolvedTransaction},
    },
    packed::{self, Byte32, CellOutput, OutPoint, Script},
    prelude::*,
};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

#[derive(Default, Clone)]
pub(crate) struct MockDataLoader {
    pub(crate) headers: HashMap<Byte32, HeaderView>,
    pub(crate) extensions: HashMap<Byte32, packed::Bytes>,
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

impl ExtensionProvider for MockDataLoader {
    fn get_block_extension(&self, hash: &Byte32) -> Option<packed::Bytes> {
        self.extensions.get(hash).cloned()
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

fn build_tx_data_with_loader(
    rtx: Arc<ResolvedTransaction>,
    data_loader: MockDataLoader,
) -> TxData<MockDataLoader> {
    let consensus = ConsensusBuilder::default().build();
    let tx_env = TxVerifyEnv::new_commit(&HeaderBuilder::default().build());

    TxData {
        rtx,
        info: Arc::new(TxInfo {
            data_loader,
            consensus: Arc::new(consensus),
            tx_env: Arc::new(tx_env),
            binaries_by_data_hash: HashMap::default(),
            binaries_by_type_hash: HashMap::default(),
            lock_groups: BTreeMap::default(),
            type_groups: BTreeMap::default(),
            outputs: Vec::new(),
        }),
    }
}

pub(crate) fn build_sg_data(
    rtx: Arc<ResolvedTransaction>,
    input_indices: Vec<usize>,
    output_indices: Vec<usize>,
) -> SgData<MockDataLoader> {
    build_sg_data_with_loader(rtx, new_mock_data_loader(), input_indices, output_indices)
}

pub(crate) fn build_sg_data_with_loader(
    rtx: Arc<ResolvedTransaction>,
    data_loader: MockDataLoader,
    input_indices: Vec<usize>,
    output_indices: Vec<usize>,
) -> SgData<MockDataLoader> {
    let tx_data = build_tx_data_with_loader(rtx, data_loader);
    let script_group = ScriptGroup {
        script: Script::default(),
        group_type: ScriptGroupType::Lock,
        input_indices,
        output_indices,
    };
    let script_hash = script_group.script.calc_script_hash();
    SgData {
        rtx: tx_data.rtx,
        tx_info: tx_data.info,
        sg_info: Arc::new(SgInfo {
            script_version: ScriptVersion::latest(),
            script_group,
            script_hash,
            program_data_piece_id: DataPieceId::CellDep(0),
        }),
    }
}

pub(crate) fn update_tx_info<F: Fn(&mut TxInfo<MockDataLoader>)>(
    mut sg_data: SgData<MockDataLoader>,
    f: F,
) -> SgData<MockDataLoader> {
    let mut tx_info = sg_data.tx_info.as_ref().clone();
    f(&mut tx_info);
    sg_data.tx_info = Arc::new(tx_info);
    sg_data
}

pub(crate) fn update_sg_info<F: Fn(&mut SgInfo)>(
    mut sg_data: SgData<MockDataLoader>,
    f: F,
) -> SgData<MockDataLoader> {
    let mut sg_info = sg_data.sg_info.as_ref().clone();
    f(&mut sg_info);
    sg_data.sg_info = Arc::new(sg_info);
    sg_data
}
