use super::super::residency::{compact_after_resolution, compact_after_verification};
use ckb_types::{
    bytes::Bytes,
    core::{TransactionBuilder, cell::CellMeta, cell::ResolvedTransaction},
    packed::CellOutput,
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
fn resolved_authority_residency_detaches_cell_views_and_data_slices() {
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
        out_point: producer
            .output_pts()
            .into_iter()
            .next()
            .expect("producer outpoint"),
        data_bytes: shared_data.len() as u64,
        mem_cell_data: Some(shared_data),
        ..Default::default()
    };
    let resolved = ResolvedTransaction {
        transaction: TransactionBuilder::default().build(),
        resolved_cell_deps: Vec::new(),
        resolved_inputs: vec![input],
        resolved_dep_groups: Vec::new(),
    };

    let compact = compact_after_resolution(resolved);
    let compact_input = &compact.resolved_inputs[0];
    assert!(!slice_is_within(
        compact_input.cell_output.as_slice(),
        producer_data.as_slice()
    ));
    assert!(!slice_is_within(
        compact_input.mem_cell_data.as_ref().expect("resident data"),
        &data_backing
    ));
    assert_eq!(compact_input.mem_cell_data.as_deref(), Some(&[0x7b; 8][..]));
}

#[test]
fn shared_verified_residency_is_preserved_without_variable_clone() {
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

    let retained = compact_after_verification(resolved);

    assert!(std::sync::Arc::ptr_eq(&retained, &shared));
    assert_eq!(
        retained.resolved_cell_deps[0]
            .mem_cell_data
            .as_ref()
            .map(Bytes::len),
        Some(dependency_data.len())
    );
}
