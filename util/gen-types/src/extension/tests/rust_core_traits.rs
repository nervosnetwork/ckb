use crate::{packed::*, prelude::*};
use ckb_fixed_hash::h256;
use numext_fixed_uint::U256;

#[test]
fn test_uint32_cmp() {
    let a: Uint32 = 10u32.into();
    let b = 20u32.into();
    let c = 10u32.into();
    assert!(a < b);
    assert!(a == c);
}

#[test]
fn test_uint64_cmp() {
    let a: Uint64 = 10u64.into();
    let b = 20u64.into();
    let c = 10u64.into();
    assert!(a < b);
    assert!(a == c);

    let a: Uint64 = 1000u64.into();
    let b: Uint64 = 2000u64.into();
    assert!(a < b);
}

#[test]
fn test_uint128_cmp() {
    let a: Uint128 = 10u128.into();
    let b = 20u128.into();
    let c = 10u128.into();
    assert!(a < b);
    assert!(a == c);
}

#[test]
fn test_uint256_cmp() {
    let a: Uint256 = U256::from(10u32).into();
    let b = U256::from(20u32).into();
    let c = U256::from(10u32).into();
    assert!(a < b);
    assert!(a == c);
}

#[test]
fn test_byte32_cmp() {
    let a: Byte32 =
        h256!("0xd1670e45af1deb9cc00951d71c09ce80932e7ddf9fb151d744436bd04ac4a562").into();
    let b = h256!("0xd2670e45af1deb9cc00951d71c09ce80932e7ddf9fb151d744436bd04ac4a562").into();
    let c = h256!("0xd1670e45af1deb9cc00951d71c09ce80932e7ddf9fb151d744436bd04ac4a562").into();

    assert!(a < b);
    assert!(a == c);
}

#[test]
fn test_bytesopt_cmp() {
    let a: BytesOpt = Some(
        Into::<Byte32>::into(h256!(
            "0xd1670e45af1deb9cc00951d71c09ce80932e7ddf9fb151d744436bd04ac4a562"
        ))
        .as_bytes(),
    )
    .into();
    let b = Some(
        Into::<Byte32>::into(h256!(
            "0xd2670e45af1deb9cc00951d71c09ce80932e7ddf9fb151d744436bd04ac4a562"
        ))
        .as_bytes(),
    )
    .into();
    let c = Some(
        Into::<Byte32>::into(h256!(
            "0xd1670e45af1deb9cc00951d71c09ce80932e7ddf9fb151d744436bd04ac4a562"
        ))
        .as_bytes(),
    )
    .into();
    let d = BytesOpt::new_builder().build();

    assert!(d.is_none());
    assert!(a < b);
    assert!(a > d);
    assert!(a == c);
}

#[test]
fn test_script_cmp() {
    let a = Script::new_builder().args([1]).build();
    let b = Script::new_builder().args([2]).build();

    assert!(a < b);
}

#[test]
fn test_celldep_cmp() {
    let a = CellDep::new_builder().dep_type(1).build();
    let b = CellDep::new_builder().dep_type(2).build();
    assert!(a < b);
}

#[test]
fn test_outpoint_cmp() {
    let a = OutPoint::new_builder().index(1u32).build();
    let b = OutPoint::new_builder().index(2u32).build();
    assert!(a < b);
}

#[test]
fn test_outpoint_borrowed_cmp_matches_the_previous_field_order() {
    let hashes = [
        [0u8; 32],
        [1u8; 32],
        [0xffu8; 32],
        std::array::from_fn(|index| index as u8),
        std::array::from_fn(|index| (31 - index) as u8),
    ];
    let indices = [0, 1, 255, 256, u32::MAX];
    let out_points = hashes
        .iter()
        .flat_map(|hash| {
            indices.iter().map(move |index| {
                OutPoint::new_builder()
                    .tx_hash(Byte32::new(*hash))
                    .index(*index)
                    .build()
            })
        })
        .collect::<Vec<_>>();

    for left in &out_points {
        for right in &out_points {
            let previous = left
                .tx_hash()
                .cmp(&right.tx_hash())
                .then_with(|| left.index().cmp(&right.index()));
            assert_eq!(left.cmp(right), previous);
        }
    }
}

#[test]
fn test_cellinput_cmp() {
    let a = CellInput::new_builder().since(1000u64).build();
    let b = CellInput::new_builder().since(2000u64).build();
    assert!(a > b);
}

#[test]
fn test_celloutput_cmp() {
    let script_lock = Script::new_builder().hash_type(1).build();
    let script_type = Script::new_builder().hash_type(2).build();
    let script_type_opt = ScriptOpt::new_builder().set(Some(script_type)).build();
    let output_a = CellOutput::new_builder().lock(script_lock.clone()).build();
    let output_b = CellOutput::new_builder()
        .lock(script_lock)
        .type_(script_type_opt)
        .build();

    assert!(output_a < output_b);
}
