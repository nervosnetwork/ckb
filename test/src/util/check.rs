use crate::Node;
use ckb_jsonrpc_types::Status;
use ckb_types::core::{BlockView, EpochNumberWithFraction, HeaderView, TransactionView};

pub fn is_transaction_pending(node: &Node, transaction: &TransactionView) -> bool {
    node.rpc_client()
        .get_transaction(transaction.hash())
        .map(|ret| ret.tx_status.status == Status::Pending && ret.cycles.is_some())
        .unwrap_or(false)
}

pub fn is_transaction_proposed(node: &Node, transaction: &TransactionView) -> bool {
    node.rpc_client()
        .get_transaction(transaction.hash())
        .map(|ret| ret.tx_status.status == Status::Proposed && ret.cycles.is_some())
        .unwrap_or(false)
}

pub fn is_transaction_committed(node: &Node, transaction: &TransactionView) -> bool {
    node.rpc_client()
        .get_transaction(transaction.hash())
        .map(|ret| ret.tx_status.status == Status::Committed && ret.cycles.is_some())
        .unwrap_or(false)
}

pub fn is_transaction_rejected(node: &Node, transaction: &TransactionView) -> bool {
    node.rpc_client()
        .get_transaction(transaction.hash())
        .map(|txstatus| txstatus.tx_status.status == Status::Rejected)
        .unwrap_or(false)
}

pub fn is_transaction_unknown(node: &Node, transaction: &TransactionView) -> bool {
    node.rpc_client()
        .get_transaction(transaction.hash())
        .map(|txstatus| txstatus.tx_status.is_unknown())
        .unwrap_or(true)
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

pub fn assert_epoch_should_less_than(node: &Node, number: u64, index: u64, length: u64) {
    let tip_header: HeaderView = node.rpc_client().get_tip_header().into();
    let tip_epoch = tip_header.epoch();
    let target_epoch = EpochNumberWithFraction::new(number, index, length);
    assert!(
        tip_epoch < target_epoch,
        "current tip epoch is {}, but expect epoch less than {}",
        tip_epoch,
        target_epoch
    );
}

pub fn assert_epoch_should_greater_than(node: &Node, number: u64, index: u64, length: u64) {
    let tip_header: HeaderView = node.rpc_client().get_tip_header().into();
    let tip_epoch = tip_header.epoch();
    let target_epoch = EpochNumberWithFraction::new(number, index, length);
    assert!(
        tip_epoch > target_epoch,
        "current tip epoch is {}, but expect epoch greater than {}",
        tip_epoch,
        target_epoch
    );
}

pub fn assert_submit_block_fail(node: &Node, block: &BlockView, message: &str) {
    let result = node
        .rpc_client()
        .submit_block("".to_owned(), block.data().into());
    assert!(
        result.is_err(),
        "expect error \"{}\" but got \"Ok(())\"",
        message,
    );
    let error = result.expect_err(&format!("block is invalid since {}", message));
    let error_string = error.to_string();
    assert!(
        error_string.contains(message),
        "expect error \"{}\" but got \"{}\"",
        message,
        error_string,
    );
}

pub fn assert_submit_block_ok(node: &Node, block: &BlockView) {
    let result = node
        .rpc_client()
        .submit_block("".to_owned(), block.data().into());
    assert!(result.is_ok(), "expect \"Ok(())\" but got \"{:?}\"", result,);
}
