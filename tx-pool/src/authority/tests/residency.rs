use super::super::residency::{accepted_resolution, compact_fixture_resolution};
use ckb_types::{
    bytes::Bytes,
    core::{
        EpochNumberWithFraction, TransactionBuilder, TransactionInfo, cell::CellMeta,
        cell::ResolvedTransaction,
    },
    packed::{Byte32, CellOutput, OutPoint},
    prelude::{Entity, Pack},
};

fn slice_is_within(inner: &[u8], outer: &[u8]) -> bool {
    let inner_start = inner.as_ptr() as usize;
    let inner_end = inner_start.saturating_add(inner.len());
    let outer_start = outer.as_ptr() as usize;
    let outer_end = outer_start.saturating_add(outer.len());
    inner_start >= outer_start && inner_end <= outer_end
}

#[test]
fn verified_entry_fixture_detaches_cell_views_and_data_slices() {
    let producer = TransactionBuilder::default()
        .output(CellOutput::default())
        .output_data(Bytes::from(vec![0x5a; 128 * 1024]).pack())
        .build();
    let producer_data = producer.data();
    let shared_output = producer.outputs().get(0).expect("producer output");
    assert!(slice_is_within(
        shared_output.as_slice(),
        producer_data.as_slice()
    ));

    let data_backing = Bytes::from(vec![0x7b; 128 * 1024]);
    let shared_data = data_backing.slice(1024..1032);
    assert!(slice_is_within(&shared_data, &data_backing));

    let input = CellMeta {
        cell_output: shared_output,
        out_point: OutPoint::new_unchecked(data_backing.slice(2048..2084)),
        transaction_info: Some(TransactionInfo::new(
            7,
            EpochNumberWithFraction::new(1, 2, 3),
            Byte32::new_unchecked(data_backing.slice(4096..4128)),
            1,
        )),
        data_bytes: shared_data.len() as u64,
        mem_cell_data: Some(shared_data),
        mem_cell_data_hash: Some(Byte32::new_unchecked(data_backing.slice(8192..8224))),
    };
    let original = input.clone();
    let resolved = ResolvedTransaction {
        transaction: TransactionBuilder::default().build(),
        resolved_cell_deps: Vec::new(),
        resolved_inputs: vec![input],
        resolved_dep_groups: Vec::new(),
    };

    let compact = compact_fixture_resolution(resolved);
    let compact_input = &compact.resolved_inputs[0];
    assert!(compact_input == &original);
    assert!(!slice_is_within(
        compact_input.cell_output.as_slice(),
        producer_data.as_slice()
    ));
    assert!(!slice_is_within(
        compact_input.mem_cell_data.as_ref().expect("resident data"),
        &data_backing
    ));
    assert_eq!(compact_input.mem_cell_data.as_deref(), Some(&[0x7b; 8][..]));
    for view in [
        compact_input.out_point.as_slice(),
        compact_input
            .transaction_info
            .as_ref()
            .expect("transaction info")
            .block_hash
            .as_slice(),
        compact_input
            .mem_cell_data_hash
            .as_ref()
            .expect("data hash")
            .as_slice(),
    ] {
        assert!(!slice_is_within(view, &data_backing));
    }
}

#[test]
fn accepted_residency_discards_dependency_payload_without_cloning_it() {
    let dependency_data = Bytes::from(vec![0x3c; 16 * 1024]);
    let dependency = CellMeta {
        cell_output: CellOutput::default(),
        mem_cell_data: Some(dependency_data.clone()),
        data_bytes: dependency_data.len() as u64,
        ..Default::default()
    };
    let resolved = std::sync::Arc::new(ResolvedTransaction {
        transaction: TransactionBuilder::default().build(),
        resolved_cell_deps: vec![dependency],
        resolved_inputs: Vec::new(),
        resolved_dep_groups: Vec::new(),
    });
    let shared = std::sync::Arc::clone(&resolved);

    let retained = accepted_resolution(&resolved);

    assert!(!std::sync::Arc::ptr_eq(&retained, &shared));
    assert_eq!(
        shared.resolved_cell_deps[0]
            .mem_cell_data
            .as_ref()
            .map(Bytes::len),
        Some(dependency_data.len())
    );
    assert_eq!(
        retained.resolved_cell_deps[0]
            .mem_cell_data
            .as_ref()
            .map(Bytes::len),
        None
    );
}
