use super::{block_offload, compact_packed};
use ckb_types::{
    bytes::Bytes,
    core::{BlockBuilder, TransactionBuilder},
    packed::{CellOutput, OutPoint},
    prelude::{Entity, Pack},
};

/// Bug #60: calling the helper from a current-thread runtime must execute
/// inline. A naked `block_in_place` panics on this runtime flavor.
#[test]
fn current_thread_runtime_executes_inline_without_panicking() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build current-thread runtime");

    let value = runtime.block_on(async { block_offload(|| 42) });
    assert_eq!(value, 42);
}

#[test]
fn compact_packed_detaches_a_small_view_from_large_backing() {
    let template = OutPoint::default();
    let entity_len = template.as_slice().len();
    let mut bytes = vec![0x5a; 8_192];
    bytes[4_096..4_096 + entity_len].copy_from_slice(template.as_slice());
    let backing = Bytes::from(bytes);
    let shared = OutPoint::new_unchecked(backing.slice(4_096..4_096 + entity_len));
    let shared_ptr = shared.as_slice().as_ptr();

    let compact = compact_packed(&shared);
    assert_eq!(compact, shared);
    assert_ne!(
        compact.as_slice().as_ptr(),
        shared_ptr,
        "a persistent packed key must not retain its parent's allocation"
    );
}

#[test]
fn compact_transaction_view_detaches_from_block_backing_without_rehashing() {
    let small = TransactionBuilder::default().build();
    let large = TransactionBuilder::default()
        .output(CellOutput::default())
        .output_data(Bytes::from(vec![0x5a; 128 * 1024]).pack())
        .build();
    let block = BlockBuilder::default()
        .transaction(small)
        .transaction(large)
        .build();
    let block_data = block.data();
    let shared = block.transactions().remove(0);
    let shared_hash = shared.hash();
    let shared_witness_hash = shared.witness_hash();
    let block_start = block_data.as_slice().as_ptr() as usize;
    let block_end = block_start + block_data.as_slice().len();
    let shared_start = shared.data().as_slice().as_ptr() as usize;
    assert!(
        shared_start >= block_start && shared_start < block_end,
        "block transaction accessor must demonstrate shared backing"
    );

    let compact = shared.into_compact();
    let compact_start = compact.data().as_slice().as_ptr() as usize;
    assert!(compact_start < block_start || compact_start >= block_end);
    assert_eq!(compact.hash(), shared_hash);
    assert_eq!(compact.witness_hash(), shared_witness_hash);
}
