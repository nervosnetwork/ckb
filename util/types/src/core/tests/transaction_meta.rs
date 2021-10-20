use crate::{
    core::{TransactionMeta, TransactionMetaBuilder},
    h256,
    packed::Byte32,
    prelude::*,
};

#[test]
fn set_unset_dead_out_of_bounds() {
    let mut meta = TransactionMeta::new(0, 0, Byte32::zero(), 4, false);
    meta.set_dead(3);
    assert!(meta.is_dead(3) == Some(true));
    meta.unset_dead(3);
    assert!(meta.is_dead(3) == Some(false));
    // none-op
    meta.set_dead(4);
    assert!(meta.is_dead(4) == None);
    meta.unset_dead(4);
    assert!(meta.is_dead(4) == None);
}

#[test]
fn test_transaction_meta_constructors() {
    let block_number = 10;
    let epoch_number = 10;
    let block_hash: Byte32 = h256!("0xf").pack();
    let outputs_count = 4;
    let mut meta1 =
        TransactionMeta::new_cellbase(block_number, epoch_number, block_hash, outputs_count, false);
    meta1.set_dead(1);
    meta1.set_dead(3);
    let meta2 = TransactionMetaBuilder::default()
        .block_number(meta1.block_number())
        .epoch_number(meta1.epoch_number())
        .block_hash(meta1.block_hash())
        .cellbase(true)
        .len(meta1.len())
        .bits(vec![0b01010000u8])
        .build();
    assert_eq!(meta1, meta2);
}
