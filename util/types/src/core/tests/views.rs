use crate::{
    bytes::Bytes,
    core::{BlockView, EpochNumberWithFraction, TransactionBuilder},
    packed,
    prelude::*,
};

#[test]
fn proposal_reader_borrows_the_transaction_hash_and_owned_output_survives_it() {
    let transaction = TransactionBuilder::default().version(123u32).build();
    let changed_witness = transaction
        .as_advanced_builder()
        .witness(Bytes::from(vec![0xbb; 1024]).pack())
        .build();
    assert_ne!(transaction.witness_hash(), changed_witness.witness_hash());
    let hash = transaction.hash();
    assert!(std::ptr::eq(transaction.hash_ref(), &transaction.hash));
    assert_eq!(transaction.hash_ref(), &hash);
    assert_eq!(transaction.hash_ref(), changed_witness.hash_ref());
    let reader = transaction.proposal_short_id_reader();
    assert_eq!(reader.as_slice(), &hash.as_slice()[..10]);
    assert_eq!(reader.as_slice().as_ptr(), hash.as_slice().as_ptr());
    assert_eq!(
        reader.as_slice(),
        changed_witness.proposal_short_id_reader().as_slice()
    );
    let owned = reader.to_entity();
    assert_eq!(owned, transaction.proposal_short_id());
    drop(transaction);
    drop(changed_witness);
    drop(hash);
    assert_eq!(owned.as_slice().len(), 10);
    packed::ProposalShortIdReader::verify(owned.as_slice(), false).unwrap();
}

#[test]
fn input_out_point_readers_borrow_only_the_canonical_input_fields() {
    let points = [
        packed::OutPoint::new(packed::Byte32::new([0x12; 32]), 0x12345678),
        packed::OutPoint::new(packed::Byte32::new([0x34; 32]), u32::MAX),
    ];
    let transaction = TransactionBuilder::default()
        .input(packed::CellInput::new(points[0].clone(), u64::MAX))
        .input(packed::CellInput::new(points[1].clone(), 0x98765432))
        .witness(Bytes::from(vec![0xaa; 1024]).pack())
        .build();
    let data = transaction.data();
    let mut readers = transaction.input_pts_reader_iter();
    assert_eq!(readers.len(), points.len());
    for point in points {
        let reader = readers.next().unwrap();
        assert_eq!(reader.as_slice(), point.as_slice());
        assert!(
            data.as_slice()
                .as_ptr_range()
                .contains(&reader.as_slice().as_ptr())
        );
    }
    assert_eq!(readers.len(), 0);
    assert!(readers.next().is_none());
    assert_eq!(
        TransactionBuilder::default()
            .build()
            .input_pts_reader_iter()
            .len(),
        0
    );
}

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
            .number(1u64)
            .epoch(EpochNumberWithFraction::new(0, 1, 1000))
            .build()
            .as_uncle();
        let uncle2 = packed::Block::new_advanced_builder()
            .number(2u64)
            .epoch(EpochNumberWithFraction::new(0, 2, 1000))
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
    let extension: packed::Bytes = [0u8, 1, 2, 3, 4, 5, 6, 7].into();
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
