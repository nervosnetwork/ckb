use crate::{packed::*, prelude::*};
use ckb_fixed_hash::h256;
use numext_fixed_uint::U256;

#[test]
fn test_uint32_cmp() {
    let a: Uint32 = 10u32.pack();
    let b = 20u32.pack();
    let c = 10u32.pack();
    assert!(a < b);
    assert!(a == c);
}

#[test]
fn test_uint64_cmp() {
    let a: Uint64 = 10u64.pack();
    let b = 20u64.pack();
    let c = 10u64.pack();
    assert!(a < b);
    assert!(a == c);

    let a: Uint64 = 1000u64.pack();
    let b: Uint64 = 2000u64.pack();
    assert!(a < b);
}

#[test]
fn test_uint128_cmp() {
    let a: Uint128 = 10u128.pack();
    let b = 20u128.pack();
    let c = 10u128.pack();
    assert!(a < b);
    assert!(a == c);
}

#[test]
fn test_uint256_cmp() {
    let a = U256::from(10u32).pack();
    let b = U256::from(20u32).pack();
    let c = U256::from(10u32).pack();
    assert!(a < b);
    assert!(a == c);
}

#[test]
fn test_byte32_cmp() {
    let a = h256!("0xd1670e45af1deb9cc00951d71c09ce80932e7ddf9fb151d744436bd04ac4a562").pack();
    let b = h256!("0xd2670e45af1deb9cc00951d71c09ce80932e7ddf9fb151d744436bd04ac4a562").pack();
    let c = h256!("0xd1670e45af1deb9cc00951d71c09ce80932e7ddf9fb151d744436bd04ac4a562").pack();

    assert!(a < b);
    assert!(a == c);
}

#[test]
fn test_bytesopt_cmp() {
    let a = Some(
        h256!("0xd1670e45af1deb9cc00951d71c09ce80932e7ddf9fb151d744436bd04ac4a562")
            .pack()
            .as_bytes(),
    )
    .pack();
    let b = Some(
        h256!("0xd2670e45af1deb9cc00951d71c09ce80932e7ddf9fb151d744436bd04ac4a562")
            .pack()
            .as_bytes(),
    )
    .pack();
    let c = Some(
        h256!("0xd1670e45af1deb9cc00951d71c09ce80932e7ddf9fb151d744436bd04ac4a562")
            .pack()
            .as_bytes(),
    )
    .pack();
    let d = BytesOpt::new_builder().build();

    assert!(d.is_none());
    assert!(a < b);
    assert!(a > d);
    assert!(a == c);
}

#[test]
fn test_script_cmp() {
    let a = Script::new_builder().args(vec![1].pack()).build();
    let b = Script::new_builder().args(vec![2].pack()).build();

    assert!(a < b);
}

#[test]
fn test_celldep_cmp() {
    let a = CellDep::new_builder().dep_type(1.into()).build();
    let b = CellDep::new_builder().dep_type(2.into()).build();
    assert!(a < b);
}

#[test]
fn test_outpoint_cmp() {
    let a = OutPoint::new_builder().index(1u32.pack()).build();
    let b = OutPoint::new_builder().index(2u32.pack()).build();
    assert!(a < b);
}

#[test]
fn test_cellinput_cmp() {
    let a = CellInput::new_builder().since(1000u64.pack()).build();
    let b = CellInput::new_builder().since(2000u64.pack()).build();
    assert!(a > b);
}

#[test]
fn test_celloutput_cmp() {
    let script_lock = Script::new_builder().hash_type(1.into()).build();
    let script_type = Script::new_builder().hash_type(2.into()).build();
    let script_type_opt = ScriptOpt::new_builder().set(Some(script_type)).build();
    let output_a = CellOutput::new_builder().lock(script_lock.clone()).build();
    let output_b = CellOutput::new_builder()
        .lock(script_lock)
        .type_(script_type_opt)
        .build();

    assert!(output_a < output_b);
}
