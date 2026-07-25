use super::detached_not_attached;
use ckb_types::{bytes::Bytes, core::TransactionBuilder, prelude::Pack};
use ckb_util::LinkedHashSet;

#[test]
fn attached_raw_hash_suppresses_detached_witness_variant() {
    let detached_variant = TransactionBuilder::default()
        .witness(Bytes::from_static(b"detached").pack())
        .build();
    let attached_variant = detached_variant
        .as_advanced_builder()
        .set_witnesses(vec![Bytes::from_static(b"attached").pack()])
        .build();
    assert_eq!(detached_variant.hash(), attached_variant.hash());
    assert_ne!(
        detached_variant.witness_hash(),
        attached_variant.witness_hash()
    );
    let unrelated = TransactionBuilder::default()
        .output_data(Bytes::from_static(b"different-raw").pack())
        .witness(Bytes::from_static(b"unrelated").pack())
        .build();
    let mut detached = LinkedHashSet::default();
    detached.extend([detached_variant, unrelated.clone()]);
    let mut attached = LinkedHashSet::default();
    attached.extend([attached_variant]);

    assert_eq!(detached_not_attached(&detached, &attached), vec![unrelated]);
}
