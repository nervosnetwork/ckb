use crate::Node;
use ckb_jsonrpc_types::Status;
use ckb_types::core::TransactionView;

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
