use crate::{packed, prelude::*};

#[test]
#[should_panic]
fn test_panic_if_an_option_should_be_some_but_not() {
    let _ = packed::BytesOpt::default().to_opt().should_be_ok();
}

#[test]
#[should_panic]
fn test_panic_if_a_molecule_result_should_be_ok_but_not() {
    let mut block = packed::Block::default().as_slice().to_vec();
    if block[0] > 0 {
        block[0] -= 1;
    } else {
        block[0] = 1;
    }
    let _ = packed::Block::from_slice(&block).should_be_ok();
}

#[test]
#[should_panic]
fn test_panic_if_molecule_from_slice_should_be_ok_but_not_1() {
    let mut block = packed::Block::default().as_slice().to_vec();
    block.push(0);
    let _ = packed::BlockReader::from_slice_should_be_ok(&block);
}

#[test]
#[should_panic]
fn test_panic_if_molecule_from_slice_should_be_ok_but_not_2() {
    let mut block = packed::Block::default().as_slice().to_vec();
    block.pop();
    let _ = packed::BlockReader::from_slice_should_be_ok(&block);
}
