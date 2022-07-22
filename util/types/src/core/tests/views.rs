use crate::{
    core::{BlockView, EpochNumberWithFraction},
    packed,
    prelude::*,
};

#[test]
fn test_block_view_convert_from_packed_block() {
    let raw_block = packed::Block::default();
    let block_unchecked = raw_block.clone().into_view_without_reset_header();
    let block = raw_block.clone().into_view();
    let raw_header = raw_block.header();
    assert_eq!(
        raw_header.as_slice(),
        block_unchecked.data().header().as_slice()
    );
    assert_ne!(raw_header.as_slice(), block.data().header().as_slice());
}

#[test]
fn test_extension_field_in_block_view() {
    let block = {
        let uncle1 = packed::Block::new_advanced_builder()
            .number(1u64.pack())
            .epoch(EpochNumberWithFraction::new(0, 1, 1000).pack())
            .build()
            .as_uncle();
        let uncle2 = packed::Block::new_advanced_builder()
            .number(2u64.pack())
            .epoch(EpochNumberWithFraction::new(0, 2, 1000).pack())
            .build()
            .as_uncle();
        packed::Block::new_advanced_builder()
            .uncle(uncle1)
            .uncle(uncle2)
            .build()
    };
    let block1 = BlockView::new_unchecked(
        block.header(),
        block.uncles(),
        block.transactions(),
        block.data().proposals(),
    );
    let extension: packed::Bytes = vec![0u8, 1, 2, 3, 4, 5, 6, 7].pack();
    // block with extension but not reset all hashes
    let block2_v1_un = BlockView::new_unchecked_with_extension(
        block.header(),
        block.uncles(),
        block.transactions(),
        block.data().proposals(),
        extension.clone(),
    );
    // block with extension and reset all hashes
    let block2_v1 = block2_v1_un.as_advanced_builder().build();
    // remove extension
    let block2_v0 = block2_v1.data().as_builder().build().into_view();

    assert_eq!(block.data().as_slice(), block1.data().as_slice(),);
    assert_eq!(block.data().as_slice(), block2_v0.data().as_slice());
    assert_ne!(block.data().as_slice(), block2_v1.data().as_slice());
    assert_ne!(block.data().as_slice(), block2_v1_un.data().as_slice());
    assert_ne!(block2_v1.data().as_slice(), block2_v1_un.data().as_slice());

    assert!(block.extension().is_none());
    assert!(block1.extension().is_none());
    assert!(block2_v0.extension().is_none());
    assert_eq!(
        extension.as_slice(),
        block2_v1.extension().unwrap().as_slice(),
    );
    assert_eq!(
        extension.as_slice(),
        block2_v1_un.extension().unwrap().as_slice(),
    );

    assert!(block.calc_extension_hash().is_none());
    assert!(block1.calc_extension_hash().is_none());
    assert!(block2_v0.calc_extension_hash().is_none());
    assert!(block2_v1.calc_extension_hash().is_some());
    assert!(block2_v1_un.calc_extension_hash().is_some());

    assert_eq!(block.extra_hash(), block.calc_uncles_hash());
    assert_eq!(block.extra_hash(), block1.calc_uncles_hash());
    assert_eq!(block.extra_hash(), block2_v0.calc_uncles_hash());
    assert_eq!(block.extra_hash(), block2_v1.calc_uncles_hash());

    assert_eq!(block.extra_hash(), block1.extra_hash());
    assert_eq!(block.extra_hash(), block2_v0.extra_hash());
    assert_ne!(block.extra_hash(), block2_v1.extra_hash());
    assert_eq!(block.extra_hash(), block2_v1_un.extra_hash());
}
