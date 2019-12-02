use super::helper::new_index_transaction;
use crate::relayer::compact_block_verifier::{PrefilledVerifier, ShortIdsVerifier};
use crate::{Status, StatusCode};
use ckb_types::packed::{CompactBlockBuilder, ProposalShortId};
use ckb_types::prelude::*;

#[test]
fn test_unordered_prefilled() {
    let prefilled = vec![0, 1, 2, 4, 3].into_iter().map(new_index_transaction);
    let block = CompactBlockBuilder::default()
        .prefilled_transactions(prefilled.pack())
        .build();
    assert_eq!(
        PrefilledVerifier::verify(&block),
        StatusCode::OutOfOrderPrefilledTransactions.into(),
    );
}

#[test]
fn test_ordered_prefilled() {
    let prefilled = (0..5).map(new_index_transaction);
    let block = CompactBlockBuilder::default()
        .prefilled_transactions(prefilled.pack())
        .build();
    assert_eq!(PrefilledVerifier::verify(&block), Status::ok());
}

#[test]
fn test_overflow_prefilled() {
    let prefilled = vec![0, 1, 2, 5].into_iter().map(new_index_transaction);
    let block = CompactBlockBuilder::default()
        .prefilled_transactions(prefilled.pack())
        .build();
    assert_eq!(
        PrefilledVerifier::verify(&block),
        StatusCode::OutOfIndexPrefilledTransactions.into(),
    );
}

#[test]
fn test_cellbase_not_prefilled() {
    let block = CompactBlockBuilder::default().build();
    assert_eq!(
        PrefilledVerifier::verify(&block),
        StatusCode::MissingPrefilledCellbase.into(),
    );

    let prefilled = (1..5).map(new_index_transaction);
    let block = CompactBlockBuilder::default()
        .prefilled_transactions(prefilled.pack())
        .build();
    assert_eq!(
        PrefilledVerifier::verify(&block),
        StatusCode::MissingPrefilledCellbase.into(),
    );
}

#[test]
fn test_duplicated_short_ids() {
    let mut short_ids: Vec<ProposalShortId> = (1..5)
        .map(|i| new_index_transaction(i).transaction().proposal_short_id())
        .collect();
    short_ids.push(short_ids[0].clone());

    let block = CompactBlockBuilder::default()
        .short_ids(short_ids.into_iter().pack())
        .build();
    assert_eq!(
        ShortIdsVerifier::verify(&block),
        StatusCode::DuplicatedShortIds.into(),
    );
}

#[test]
fn test_intersected_short_ids() {
    let prefilled = (0..=5).map(new_index_transaction);
    let short_ids = (5..9).map(|i| new_index_transaction(i).transaction().proposal_short_id());

    let block = CompactBlockBuilder::default()
        .prefilled_transactions(prefilled.pack())
        .short_ids(short_ids.pack())
        .build();
    assert_eq!(
        ShortIdsVerifier::verify(&block),
        StatusCode::DuplicatedPrefilledTransactions.into(),
    );
}

#[test]
fn test_normal() {
    let prefilled = vec![1, 2, 5].into_iter().map(new_index_transaction);
    let short_ids = vec![0, 3, 4]
        .into_iter()
        .map(|i| new_index_transaction(i).transaction().proposal_short_id());
    let block = CompactBlockBuilder::default()
        .prefilled_transactions(prefilled.pack())
        .short_ids(short_ids.pack())
        .build();
    assert_eq!(ShortIdsVerifier::verify(&block), Status::ok());
}
