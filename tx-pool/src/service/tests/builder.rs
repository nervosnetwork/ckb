use super::*;
use crate::service::{AsyncRequest, BoundedTransaction, NotifyTxBatch};
use ckb_network::PeerIndex;
use ckb_types::{
    bytes::Bytes,
    core::{TransactionBuilder, TransactionView},
    prelude::Pack,
};

fn transaction(payload_bytes: usize) -> TransactionView {
    TransactionBuilder::default()
        .output_data(Bytes::from(vec![0; payload_bytes]).pack())
        .build()
}

fn remote_message(
    transaction: TransactionView,
    peer: PeerIndex,
) -> (Message, tokio::sync::oneshot::Receiver<()>) {
    let (responder, response) = tokio::sync::oneshot::channel();
    (
        Message::SubmitRemoteTx(AsyncRequest::call(
            RemoteTxSubmission::new(
                BoundedTransaction::try_new(transaction)
                    .expect("the remote fixture transaction is bounded"),
                0,
                peer,
            ),
            responder,
        )),
        response,
    )
}

fn start_batch(message: Message) -> RetainedIngressBatch {
    match RetainedIngressBatch::try_new(message) {
        Ok(batch) => batch,
        Err(_) => panic!("fixture message starts a retained-ingress batch"),
    }
}

#[test]
fn retained_ingress_batch_groups_only_the_same_remote_peer() {
    let first_peer = PeerIndex::from(51);
    let second_peer = PeerIndex::from(52);
    let (first, _first_response) = remote_message(transaction(1), first_peer);
    let mut batch = start_batch(first);
    let (same_peer, _same_response) = remote_message(transaction(2), first_peer);
    assert!(matches!(
        batch.append(same_peer),
        RetainedIngressAppend::Consumed
    ));

    let (other_peer, _other_response) = remote_message(transaction(3), second_peer);
    let RetainedIngressAppend::Lookahead(Message::SubmitRemoteTx(lookahead)) =
        batch.append(other_peer)
    else {
        panic!("a different peer remains one exact dispatcher lookahead");
    };
    assert_eq!(lookahead.arguments.peer, second_peer);
    assert!(matches!(
        batch,
        RetainedIngressBatch::Remote {
            peer,
            ref submissions,
            ref responders,
            ..
        } if peer == first_peer && submissions.len() == 2 && responders.len() == 2
    ));
}

#[test]
fn retained_ingress_batch_keeps_a_nonfitting_proposal_message_whole() {
    let first = transaction(500_000);
    let second = transaction(300_000);
    let third = transaction(300_000);
    let initial = Message::NotifyTxs(Notify::new(
        NotifyTxBatch::try_new(vec![first]).expect("initial proposal carrier is bounded"),
    ));
    let mut batch = start_batch(initial);
    let next = Message::NotifyTxs(Notify::new(
        NotifyTxBatch::try_new(vec![second, third])
            .expect("the next proposal carrier is independently bounded"),
    ));
    let RetainedIngressAppend::Lookahead(Message::NotifyTxs(Notify { arguments })) =
        batch.append(next)
    else {
        panic!("a nonfitting proposal message remains the exact lookahead");
    };
    assert_eq!(arguments.transactions.len(), 2);
    assert!(matches!(
        batch,
        RetainedIngressBatch::Proposal {
            ref transactions,
            bytes,
        } if transactions.len() == 1 && bytes <= RETAINED_INGRESS_BYTES
    ));
}

#[test]
fn retained_ingress_batch_appends_a_complete_fitting_proposal_message() {
    let initial = Message::NotifyTxs(Notify::new(
        NotifyTxBatch::try_new(vec![transaction(100_000)])
            .expect("initial proposal carrier is bounded"),
    ));
    let mut batch = start_batch(initial);
    let next = Message::NotifyTxs(Notify::new(
        NotifyTxBatch::try_new(vec![transaction(100_000), transaction(100_000)])
            .expect("the next proposal carrier is independently bounded"),
    ));
    assert!(matches!(
        batch.append(next),
        RetainedIngressAppend::Consumed
    ));
    assert!(matches!(
        batch,
        RetainedIngressBatch::Proposal {
            ref transactions,
            bytes,
        } if transactions.len() == 3 && bytes <= RETAINED_INGRESS_BYTES
    ));
}

#[test]
fn retained_ingress_batch_preserves_handler_concurrency_at_the_apply_bound() {
    let initial = Message::NotifyTxs(Notify::new(
        NotifyTxBatch::try_new(
            (0..RETAINED_INGRESS_APPLY_ITEMS - 1)
                .map(|_| transaction(0))
                .collect(),
        )
        .expect("the initial proposal carrier is bounded"),
    ));
    let mut batch = start_batch(initial);
    let next = Message::NotifyTxs(Notify::new(
        NotifyTxBatch::try_new(vec![transaction(0), transaction(0)])
            .expect("the next proposal carrier is independently bounded"),
    ));
    let RetainedIngressAppend::Lookahead(Message::NotifyTxs(Notify { arguments })) =
        batch.append(next)
    else {
        panic!("a message that crosses one Apply remains available to another handler");
    };
    assert_eq!(arguments.transactions.len(), 2);
    assert!(matches!(
        batch,
        RetainedIngressBatch::Proposal {
            ref transactions,
            ..
        } if transactions.len() == RETAINED_INGRESS_APPLY_ITEMS - 1
    ));
}

#[test]
fn retained_ingress_batch_remote_apply_bound_is_exact() {
    let peer = PeerIndex::from(53);
    let (first, _response) = remote_message(transaction(0), peer);
    let mut batch = start_batch(first);
    for _ in 1..RETAINED_INGRESS_APPLY_ITEMS {
        let (message, _response) = remote_message(transaction(0), peer);
        assert!(matches!(
            batch.append(message),
            RetainedIngressAppend::Consumed
        ));
    }
    assert!(!batch.can_drain());
    assert!(matches!(
        batch,
        RetainedIngressBatch::Remote {
            ref submissions,
            ref responders,
            ..
        } if submissions.len() == RETAINED_INGRESS_APPLY_ITEMS
            && responders.len() == RETAINED_INGRESS_APPLY_ITEMS
    ));
}

#[test]
fn empty_proposal_notification_does_not_create_an_authority_batch() {
    let message = Message::NotifyTxs(Notify::new(
        NotifyTxBatch::try_new(Vec::new()).expect("empty notification is a valid no-op"),
    ));
    assert!(RetainedIngressBatch::try_new(message).is_err());
}
