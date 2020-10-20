use crate::util::check::is_transaction_committed;
use crate::Node;
use ckb_types::core::EpochNumberWithFraction;
use ckb_types::{core::TransactionView, packed::OutPoint};

/// Send the given transaction and make it committed
pub(crate) fn ensure_committed(node: &Node, transaction: &TransactionView) -> OutPoint {
    let commit_elapsed = node.consensus().tx_proposal_window().closest() as usize + 2;
    node.rpc_client()
        .send_transaction(transaction.data().into());
    node.generate_blocks(commit_elapsed);
    assert!(is_transaction_committed(node, transaction));
    OutPoint::new(transaction.hash(), 0)
}

/// A helper function keep the node growing until into the target EpochNumberWithFraction.
pub(crate) fn goto_target_point(node: &Node, target_point: EpochNumberWithFraction) {
    loop {
        let tip_epoch = node.rpc_client().get_tip_header().inner.epoch;
        let tip_point = EpochNumberWithFraction::from_full_value(tip_epoch.value());

        // Here is our target EpochNumberWithFraction.
        if tip_point >= target_point {
            break;
        }

        node.generate_block();
    }
}
