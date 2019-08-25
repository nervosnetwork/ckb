use crate::relayer::block_proposal_process::{BlockProposalProcess, Status};
use crate::relayer::tests::helper::{build_chain, new_transaction, MockProtocalContext};
use ckb_types::packed::{self, ProposalShortId};
use ckb_types::prelude::*;
use std::sync::Arc;

#[test]
fn test_no_unknown() {
    let (relayer, always_success_out_point) = build_chain(5);

    let transaction = new_transaction(&relayer, 1, &always_success_out_point);

    let transactions = vec![transaction.clone()];

    // known tx
    {
        relayer
            .shared
            .mark_as_known_tx(transaction.hash().to_owned());
    }
    let content = packed::BlockProposal::new_builder()
        .transactions(transactions.into_iter().map(|tx| tx.data()).pack())
        .build();

    let mock_protocal_context = MockProtocalContext::default();
    let nc = Arc::new(mock_protocal_context);

    let process = BlockProposalProcess::new(
        content.as_reader(),
        &relayer,
        Arc::<MockProtocalContext>::clone(&nc),
    );
    let r = process.execute();
    assert_eq!(r.ok(), Some(Status::NoUnknown));
}

#[test]
fn test_no_asked() {
    let (relayer, always_success_out_point) = build_chain(5);

    let transaction = new_transaction(&relayer, 1, &always_success_out_point);

    let transactions = vec![transaction.clone()];

    let content = packed::BlockProposal::new_builder()
        .transactions(transactions.into_iter().map(|tx| tx.data()).pack())
        .build();

    let mock_protocal_context = MockProtocalContext::default();
    let nc = Arc::new(mock_protocal_context);

    let process = BlockProposalProcess::new(
        content.as_reader(),
        &relayer,
        Arc::<MockProtocalContext>::clone(&nc),
    );
    let r = process.execute();
    assert_eq!(r.ok(), Some(Status::NoAsked));

    let known = relayer.shared.already_known_tx(&transaction.hash());
    assert_eq!(known, false);
}

#[test]
fn test_ok() {
    let (relayer, always_success_out_point) = build_chain(5);

    let transaction = new_transaction(&relayer, 1, &always_success_out_point);
    let transactions = vec![transaction.clone()];
    let proposals: Vec<ProposalShortId> = transactions
        .iter()
        .map(|tx| tx.proposal_short_id())
        .collect();

    // Before asked proposals
    {
        relayer.shared.insert_inflight_proposals(proposals);
    }

    let content = packed::BlockProposal::new_builder()
        .transactions(transactions.into_iter().map(|tx| tx.data()).pack())
        .build();

    let mock_protocal_context = MockProtocalContext::default();
    let nc = Arc::new(mock_protocal_context);

    let process = BlockProposalProcess::new(
        content.as_reader(),
        &relayer,
        Arc::<MockProtocalContext>::clone(&nc),
    );
    let r = process.execute();
    assert_eq!(r.ok(), Some(Status::Ok));

    let known = relayer.shared.already_known_tx(&transaction.hash());
    assert_eq!(known, true);
}
