use crate::types::TransactionFilter;
use ckb_core::transaction::TransactionBuilder;

#[test]
fn transaction_filter() {
    let mut filter = TransactionFilter::new(&vec![0; 8], 3, 1);
    let tx = TransactionBuilder::default().build();
    assert!(!filter.contains(&tx));
    filter.insert(&tx.hash());
    assert!(filter.contains(&tx));
}
