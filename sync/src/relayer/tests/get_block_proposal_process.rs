use crate::relayer::get_block_proposal_process::GetBlockProposalProcess;
use crate::relayer::tests::helper::{build_chain, new_transaction, MockProtocolContext};
use crate::StatusCode;
use ckb_network::{PeerIndex, SupportProtocols};
use ckb_types::packed;
use ckb_types::prelude::*;
use std::sync::Arc;

#[test]
fn test_duplicate() {
    let (relayer, always_success_out_point) = build_chain(5);

    let tx = new_transaction(&relayer, 1, &always_success_out_point);
    let id = tx.proposal_short_id();
    let snapshot = relayer.shared.shared().snapshot();
    let hash = snapshot.tip_header().hash();

    let content = packed::GetBlockProposal::new_builder()
        .block_hash(hash)
        .proposals(vec![id.clone(), id].into_iter().pack())
        .build();
    let mock_protocol_context = MockProtocolContext::new(SupportProtocols::Relay);
    let nc = Arc::new(mock_protocol_context);
    let peer_index: PeerIndex = 1.into();
    let process = GetBlockProposalProcess::new(content.as_reader(), &relayer, nc, peer_index);

    assert_eq!(
        process.execute(),
        StatusCode::RequestDuplicate.with_context("Request duplicate proposal")
    );
}
