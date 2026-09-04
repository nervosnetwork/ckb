//! Direct regression for the Ready cost coordinate used by production ordering.

use super::foundation::{
    limits, resolved_payload_with_facts, verify_remote_transaction_with_payload,
};
use crate::authority::{
    plan::TxPoolAuthority,
    state::{OwnedTx, PreAcceptedPhase},
};
use ckb_types::{
    bytes::Bytes,
    core::{Capacity, TransactionBuilder, TransactionView},
    packed::CellOutput,
    prelude::Pack,
};

fn transaction(version: u32, payload_bytes: usize) -> TransactionView {
    TransactionBuilder::default()
        .version(version)
        .output(CellOutput::default())
        .output_data(Bytes::from(vec![version as u8; payload_bytes]).pack())
        .build()
}

#[test]
fn uak_ready_order_uses_block_serialized_bytes_not_raw_payload_bytes() {
    let transactions = [transaction(1, 0), transaction(2, 512)];
    let payload_bytes = transactions.each_ref().map(|transaction| {
        u64::try_from(transaction.data().total_size()).expect("fixture size fits u64")
    });
    let fees = payload_bytes.map(|bytes| bytes * 2 + 4);
    assert!(
        u128::from(fees[0]) * u128::from(payload_bytes[1])
            > u128::from(fees[1]) * u128::from(payload_bytes[0])
    );
    let serialized_bytes = payload_bytes.map(|bytes| bytes + 4);
    assert!(
        u128::from(fees[0]) * u128::from(serialized_bytes[1])
            < u128::from(fees[1]) * u128::from(serialized_bytes[0])
    );

    let mut authority = TxPoolAuthority::for_foundation(limits());
    let hashes = transactions
        .iter()
        .cloned()
        .zip(fees)
        .enumerate()
        .map(|(index, (transaction, fee))| {
            let payload = resolved_payload_with_facts(
                &transaction,
                Vec::new(),
                Vec::new(),
                Capacity::shannons(fee),
            );
            verify_remote_transaction_with_payload(
                &mut authority,
                transaction,
                1_000 + index,
                payload,
            )
        })
        .collect::<Vec<_>>();

    for (hash, transaction) in hashes.iter().zip(&transactions) {
        let owner = authority.entry(hash).expect("Ready owner exists");
        let OwnedTx::PreAccepted(entry) = &owner else {
            panic!("fixture owner is preaccepted")
        };
        let PreAcceptedPhase::Ready(verified) = &entry.phase else {
            panic!("fixture owner reached Ready")
        };
        assert_eq!(
            verified.metrics().cost.serialized_bytes,
            transaction.data().serialized_size_in_block()
        );
    }
    assert_eq!(
        authority
            .ready_for_reference()
            .into_iter()
            .map(|(hash, _)| hashes
                .iter()
                .position(|candidate| candidate == &hash)
                .unwrap())
            .collect::<Vec<_>>(),
        [1, 0]
    );
}
