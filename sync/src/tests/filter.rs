use crate::types::TransactionFilter;
use ckb_core::transaction::{CellInput, CellOutput, OutPoint, Transaction, TransactionBuilder};
use numext_fixed_hash::H256;

#[test]
fn transaction_filter() {
    let mut filter = TransactionFilter::new(&vec![0; 8], 3, 1);
    let tx = TransactionBuilder::default().build();
    assert!(!filter.contains(&tx));
    filter.insert(&tx.hash());
    assert!(filter.contains(&tx));
}
