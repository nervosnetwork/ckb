use crate::Status;
use crate::StatusCode;
use crate::relayer::MAX_RELAY_TXS_BYTES_PER_BATCH;
use crate::relayer::block_proposal_process::BlockProposalProcess;
use crate::relayer::tests::helper::{build_chain, new_transaction};
use crate::relayer::transaction_hashes_process::TransactionHashesProcess;
use ckb_network::PeerIndex;
use ckb_types::bytes::Bytes;
use ckb_types::packed::{self, ProposalShortId};
use ckb_types::prelude::*;
use std::time::{Duration, Instant};

#[test]
fn test_no_unknown() {
    let (_chain, relayer, always_success_out_point) = build_chain(5);
    let transaction = new_transaction(&relayer, 1, &always_success_out_point);

    let transactions = vec![transaction.clone()];

    // known tx
    {
        relayer.shared.state().mark_as_known_tx(transaction.hash());
    }
    let content = packed::BlockProposal::new_builder()
        .transactions(
            transactions
                .into_iter()
                .map(|tx| tx.data())
                .collect::<Vec<_>>(),
        )
        .build();

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let process = BlockProposalProcess::new(content.as_reader(), &relayer);
    assert_eq!(rt.block_on(process.execute()), Status::ignored());
}

#[test]
fn test_no_asked() {
    let (_chain, relayer, always_success_out_point) = build_chain(5);
    let transaction = new_transaction(&relayer, 1, &always_success_out_point);

    let transactions = vec![transaction.clone()];

    let content = packed::BlockProposal::new_builder()
        .transactions(
            transactions
                .into_iter()
                .map(|tx| tx.data())
                .collect::<Vec<_>>(),
        )
        .build();

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let process = BlockProposalProcess::new(content.as_reader(), &relayer);
    assert_eq!(rt.block_on(process.execute()), Status::ignored());

    let known = relayer.shared.state().already_known_tx(&transaction.hash());
    assert!(!known);
}

#[test]
fn test_ok() {
    let (_chain, relayer, always_success_out_point) = build_chain(5);
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
        .transactions(
            transactions
                .into_iter()
                .map(|tx| tx.data())
                .collect::<Vec<_>>(),
        )
        .build();

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let process = BlockProposalProcess::new(content.as_reader(), &relayer);
    assert_eq!(rt.block_on(process.execute()), Status::ok());

    let known = relayer.shared.state().already_known_tx(&transaction.hash());
    assert!(known);
}

#[test]
fn test_oversized_batch_is_rejected_before_relay_state_changes() {
    let (_chain, relayer, always_success_out_point) = build_chain(5);
    let transaction = new_transaction(&relayer, 1, &always_success_out_point)
        .as_advanced_builder()
        .set_outputs_data(vec![
            Bytes::from(vec![0; MAX_RELAY_TXS_BYTES_PER_BATCH]).pack(),
        ])
        .build();
    let proposal = transaction.proposal_short_id();
    relayer
        .shared
        .state()
        .insert_inflight_proposals(vec![proposal.clone()], 1);

    let content = packed::BlockProposal::new_builder()
        .transactions(vec![transaction.data()])
        .build();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let process = BlockProposalProcess::new(content.as_reader(), &relayer);

    assert_eq!(
        rt.block_on(process.execute()),
        StatusCode::ProtocolMessageIsMalformed.into(),
    );
    assert!(relayer.shared.state().contains_inflight_proposal(&proposal));
    assert!(!relayer.shared.state().already_known_tx(&transaction.hash()));
}

#[test]
fn test_clear_expired_inflight_proposals() {
    // mark the inflight proposals as block number 2, the default farthest proposal window is 10, it will be expired and ignored
    let (_chain, relayer, always_success_out_point) = build_chain(13);
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
        .transactions(
            transactions
                .into_iter()
                .map(|tx| tx.data())
                .collect::<Vec<_>>(),
        )
        .build();

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let process = BlockProposalProcess::new(content.as_reader(), &relayer);
    assert_eq!(rt.block_on(process.execute()), Status::ignored());
}

/// Negative production refinement witness for the Proposal pre-authority
/// handoff. The required closed-channel observation is one exact restoration
/// of both in-flight and known state. Current code consumes both projections
/// before controller acknowledgement. M4 must invert this witness to equality.
#[test]
fn counterexample_proposal_closed_controller_consumes_inflight_and_marks_known() {
    let (_chain, relayer, always_success_out_point) = build_chain(1);
    let transaction = new_transaction(&relayer, 702, &always_success_out_point);
    let hash = transaction.hash();
    let proposal = transaction.proposal_short_id();
    let state = relayer.shared.state();
    state.insert_inflight_proposals(vec![proposal.clone()], 1);

    let controller = relayer.shared.shared().tx_pool_controller();
    let start_deadline = Instant::now() + Duration::from_secs(5);
    while !controller.service_started() && Instant::now() < start_deadline {
        std::thread::yield_now();
    }
    assert!(
        controller.service_started(),
        "the fixture tx-pool reaches Running"
    );
    controller.stop();
    let stop_deadline = Instant::now() + Duration::from_secs(5);
    while controller.service_started() && Instant::now() < stop_deadline {
        std::thread::yield_now();
    }
    assert!(
        !controller.service_started(),
        "the fixture controller stops"
    );
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("the proposal boundary runtime builds");
    assert!(
        rt.block_on(controller.notify_txs_async(vec![transaction.clone()]))
            .expect_err("the stopped controller has no notification owner")
            .to_string()
            .contains("channel closed")
    );

    let content = packed::BlockProposal::new_builder()
        .transactions(vec![transaction.data()])
        .build();
    let process = BlockProposalProcess::new(content.as_reader(), &relayer);
    assert_eq!(rt.block_on(process.execute()), Status::ok());
    assert!(!state.contains_inflight_proposal(&proposal));
    assert!(state.already_known_tx(&hash));

    let replacement_peer = PeerIndex::from(8usize);
    let announcement = packed::RelayTransactionHashes::new_builder()
        .tx_hashes(vec![hash])
        .build();
    let _ = TransactionHashesProcess::new(announcement.as_reader(), &relayer, replacement_peer)
        .execute();
    assert!(
        !state.pop_ask_for_txs().contains_key(&replacement_peer),
        "the stale known projection suppresses the same raw transaction"
    );
}
