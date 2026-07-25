use super::detach_parent_hashes;
use ckb_types::{bytes::Bytes, packed::Byte32, prelude::Entity};
use std::collections::HashSet;

#[test]
fn parent_wait_hashes_do_not_retain_the_source_backing() {
    let backing = Bytes::from(vec![7u8; 4_096]);
    let shared = Byte32::new_unchecked(backing.slice(2_048..2_080));
    let shared_ptr = shared.as_slice().as_ptr();

    let detached = detach_parent_hashes(HashSet::from([shared]));
    let detached = detached.iter().next().expect("one detached parent");

    assert_eq!(detached.as_slice(), &[7u8; 32]);
    assert_ne!(
        detached.as_slice().as_ptr(),
        shared_ptr,
        "the outbox/coordinator hash must own a compact allocation"
    );
    drop(backing);
}
