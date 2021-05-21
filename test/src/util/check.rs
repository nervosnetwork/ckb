use crate::Node;
use ckb_jsonrpc_types::Status;
use ckb_types::core::{EpochNumberWithFraction, HeaderView, TransactionView};

pub fn is_transaction_pending(node: &Node, transaction: &TransactionView) -> bool {
    node.rpc_client()
        .get_transaction(transaction.hash())
        .map(|txstatus| txstatus.tx_status.status == Status::Pending)
        .unwrap_or(false)
}

pub fn is_transaction_proposed(node: &Node, transaction: &TransactionView) -> bool {
    node.rpc_client()
        .get_transaction(transaction.hash())
        .map(|txstatus| txstatus.tx_status.status == Status::Proposed)
        .unwrap_or(false)
}

pub fn is_transaction_committed(node: &Node, transaction: &TransactionView) -> bool {
    node.rpc_client()
        .get_transaction(transaction.hash())
        .map(|txstatus| txstatus.tx_status.status == Status::Committed)
        .unwrap_or(false)
}

pub fn is_transaction_unknown(node: &Node, transaction: &TransactionView) -> bool {
    node.rpc_client()
        .get_transaction(transaction.hash())
        .is_none()
}

pub fn assert_epoch_should_be(node: &Node, number: u64, index: u64, length: u64) {
    let tip_header: HeaderView = node.rpc_client().get_tip_header().into();
    let tip_epoch = tip_header.epoch();
    let target_epoch = EpochNumberWithFraction::new(number, index, length);
    assert_eq!(
        tip_epoch, target_epoch,
        "current tip epoch is {}, but expect epoch {}",
        tip_epoch, target_epoch
    );
}
