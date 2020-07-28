use crate::Node;
use ckb_types::{
    bytes::Bytes,
    core::{Capacity, TransactionView},
    prelude::*,
};

pub fn new_transaction_with_fee_and_size(
    node: &Node,
    parent_tx: &TransactionView,
    fee: Capacity,
    tx_size: usize,
) -> TransactionView {
    let input_capacity: Capacity = parent_tx
        .outputs()
        .get(0)
        .expect("parent output")
        .capacity()
        .unpack();
    let capacity = input_capacity.safe_sub(fee).unwrap();
    let tx = node.new_transaction_with_since_capacity(parent_tx.hash(), 0, capacity);
    let original_tx_size = tx.data().serialized_size_in_block();
    let tx = tx
        .as_advanced_builder()
        .set_outputs_data(vec![
            Bytes::from(vec![0u8; tx_size - original_tx_size]).pack()
        ])
        .build();
    assert_eq!(
        tx.data().serialized_size_in_block(),
        tx_size,
        "tx size incorrect"
    );
    tx
}
