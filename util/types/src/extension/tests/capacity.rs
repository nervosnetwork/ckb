use crate::{
    core::{capacity_bytes, Capacity},
    packed,
    prelude::*,
};

#[test]
fn script_occupied_capacity() {
    let testcases = vec![
        (vec![], 32 + 1),
        (vec![0], 1 + 32 + 1),
        (vec![1], 1 + 32 + 1),
        (vec![0, 0], 2 + 32 + 1),
    ];
    for (args, ckb) in testcases.into_iter() {
        let script = packed::Script::new_builder().args(args.pack()).build();
        let expect = Capacity::bytes(ckb).unwrap();
        assert_eq!(script.occupied_capacity().unwrap(), expect);
    }
}

#[test]
fn min_cell_output_capacity() {
    let lock = packed::Script::new_builder().build();
    let output = packed::CellOutput::new_builder().lock(lock).build();
    assert_eq!(
        output.occupied_capacity(Capacity::zero()).unwrap(),
        capacity_bytes!(41)
    );
}

#[test]
fn min_secp256k1_cell_output_capacity() {
    let lock = packed::Script::new_builder()
        .args(vec![0u8; 20].pack())
        .build();
    let output = packed::CellOutput::new_builder().lock(lock).build();
    assert_eq!(
        output.occupied_capacity(Capacity::zero()).unwrap(),
        capacity_bytes!(61)
    );
}
