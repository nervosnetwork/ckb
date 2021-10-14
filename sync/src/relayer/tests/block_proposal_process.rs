use crate::relayer::block_proposal_process::BlockProposalProcess;
use crate::relayer::tests::helper::{build_chain, new_transaction};
use crate::Status;
use ckb_types::packed::{self, ProposalShortId};
use ckb_types::prelude::*;

#[test]
fn test_no_unknown() {
    let (relayer, always_success_out_point) = build_chain(5);
    let transaction = new_transaction(&relayer, 1, &always_success_out_point);

    let transactions = vec![transaction.clone()];

    // known tx
    {
        relayer.shared.state().mark_as_known_tx(transaction.hash());
    }
    let content = packed::BlockProposal::new_builder()
        .transactions(transactions.into_iter().map(|tx| tx.data()).pack())
        .build();

    let process = BlockProposalProcess::new(content.as_reader(), &relayer);
    assert_eq!(process.execute(), Status::ignored());
}

#[test]
fn test_no_asked() {
    let (relayer, always_success_out_point) = build_chain(5);
    let transaction = new_transaction(&relayer, 1, &always_success_out_point);

    let transactions = vec![transaction.clone()];

    let content = packed::BlockProposal::new_builder()
        .transactions(transactions.into_iter().map(|tx| tx.data()).pack())
        .build();

    let process = BlockProposalProcess::new(content.as_reader(), &relayer);
    assert_eq!(process.execute(), Status::ignored());

    let known = relayer.shared.state().already_known_tx(&transaction.hash());
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
        relayer
            .shared
            .state()
            .insert_inflight_proposals(proposals, 1);
    }

    let content = packed::BlockProposal::new_builder()
        .transactions(transactions.into_iter().map(|tx| tx.data()).pack())
        .build();

    let process = BlockProposalProcess::new(content.as_reader(), &relayer);
    assert_eq!(process.execute(), Status::ok());

    let known = relayer.shared.state().already_known_tx(&transaction.hash());
    assert_eq!(known, true);
}

#[test]
fn test_clear_expired_inflight_proposals() {
    // mark the inflight proposals as block number 2, the default farthest proposal window is 10, it will be expired and ignored
    let (relayer, always_success_out_point) = build_chain(13);
    let transaction = new_transaction(&relayer, 1, &always_success_out_point);
    let transactions = vec![transaction];
    let proposals: Vec<ProposalShortId> = transactions
        .iter()
        .map(|tx| tx.proposal_short_id())
        .collect();

    {
        relayer
            .shared
            .state()
            .insert_inflight_proposals(proposals, 2);
    }

    let content = packed::BlockProposal::new_builder()
        .transactions(transactions.into_iter().map(|tx| tx.data()).pack())
        .build();

    let process = BlockProposalProcess::new(content.as_reader(), &relayer);
    assert_eq!(process.execute(), Status::ignored());
}
