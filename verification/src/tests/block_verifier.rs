use super::super::block_verifier::{
    BlockBytesVerifier, BlockProposalsLimitVerifier, CellbaseVerifier,
};
use super::super::error::{CellbaseError, Error as VerifyError};
use ckb_core::block::BlockBuilder;
use ckb_core::header::HeaderBuilder;
use ckb_core::script::Script;
use ckb_core::transaction::{
    CellInput, CellOutput, OutPoint, ProposalShortId, Transaction, TransactionBuilder,
};
use ckb_core::{capacity_bytes, Bytes, Capacity};
use numext_fixed_hash::{h256, H256};

fn create_cellbase_transaction_with_capacity(capacity: Capacity) -> Transaction {
    TransactionBuilder::default()
        .input(CellInput::new_cellbase_input())
        .output(CellOutput::new(
            capacity,
            (&[0; 8][..]).into(),
            Script::default(),
            None,
        ))
        .build()
}

fn create_cellbase_transaction() -> Transaction {
    create_cellbase_transaction_with_capacity(capacity_bytes!(100))
}

fn create_normal_transaction() -> Transaction {
    TransactionBuilder::default()
        .input(CellInput::new(OutPoint::new_cell(h256!("0x1"), 0), 0))
        .output(CellOutput::new(
            capacity_bytes!(100),
            Bytes::default(),
            Script::default(),
            None,
        ))
        .build()
}

#[test]
pub fn test_block_without_cellbase() {
    let block = BlockBuilder::default()
        .transaction(TransactionBuilder::default().build())
        .build();
    let verifier = CellbaseVerifier::new();
    assert_eq!(
        verifier.verify(&block),
        Err(VerifyError::Cellbase(CellbaseError::InvalidQuantity))
    );
}

#[test]
pub fn test_block_with_one_cellbase_at_first() {
    let transaction = create_normal_transaction();

    let block = BlockBuilder::default()
        .transaction(create_cellbase_transaction())
        .transaction(transaction)
        .build();

    let verifier = CellbaseVerifier::new();
    assert!(verifier.verify(&block).is_ok());
}

#[test]
pub fn test_block_with_one_cellbase_at_last() {
    let block = BlockBuilder::default()
        .transaction(create_normal_transaction())
        .transaction(create_cellbase_transaction())
        .build();

    let verifier = CellbaseVerifier::new();
    assert_eq!(
        verifier.verify(&block),
        Err(VerifyError::Cellbase(CellbaseError::InvalidPosition))
    );
}

#[test]
pub fn test_block_with_two_cellbases() {
    let block = BlockBuilder::default()
        .transaction(create_cellbase_transaction())
        .transaction(create_cellbase_transaction())
        .build();

    let verifier = CellbaseVerifier::new();
    assert_eq!(
        verifier.verify(&block),
        Err(VerifyError::Cellbase(CellbaseError::InvalidQuantity))
    );
}

#[test]
pub fn test_cellbase_with_less_reward() {
    let transaction = create_normal_transaction();

    let block = BlockBuilder::default()
        .transaction(create_cellbase_transaction_with_capacity(capacity_bytes!(
            50
        )))
        .transaction(transaction)
        .build();

    let verifier = CellbaseVerifier::new();
    assert!(verifier.verify(&block).is_ok());
}

#[test]
pub fn test_cellbase_with_fee() {
    let transaction = create_normal_transaction();

    let block = BlockBuilder::default()
        .transaction(create_cellbase_transaction_with_capacity(capacity_bytes!(
            110
        )))
        .transaction(transaction)
        .build();

    let verifier = CellbaseVerifier::new();
    assert!(verifier.verify(&block).is_ok());
}

#[test]
pub fn test_max_block_bytes_verifier_skip_genesis() {
    let block = BlockBuilder::default().build();
    let proof_size = 0usize;

    {
        let verifier =
            BlockBytesVerifier::new(block.serialized_size(proof_size) as u64, proof_size);
        assert_eq!(verifier.verify(&block), Ok(()));
    }

    {
        let verifier =
            BlockBytesVerifier::new(block.serialized_size(proof_size) as u64 - 1, proof_size);
        assert_eq!(verifier.verify(&block), Ok(()),);
    }
}

#[test]
pub fn test_max_block_bytes_verifier() {
    let block = BlockBuilder::from_header_builder(HeaderBuilder::default().number(2)).build();
    let proof_size = 0usize;

    {
        let verifier =
            BlockBytesVerifier::new(block.serialized_size(proof_size) as u64, proof_size);
        assert_eq!(verifier.verify(&block), Ok(()));
    }

    {
        let verifier =
            BlockBytesVerifier::new(block.serialized_size(proof_size) as u64 - 1, proof_size);
        assert_eq!(
            verifier.verify(&block),
            Err(VerifyError::ExceededMaximumBlockBytes)
        );
    }
}

#[test]
pub fn test_max_proposals_limit_verifier() {
    let block = BlockBuilder::default()
        .proposal(ProposalShortId::zero())
        .build();

    {
        let verifier = BlockProposalsLimitVerifier::new(1);
        assert_eq!(verifier.verify(&block), Ok(()));
    }

    {
        let verifier = BlockProposalsLimitVerifier::new(0);
        assert_eq!(
            verifier.verify(&block),
            Err(VerifyError::ExceededMaximumProposalsLimit)
        );
    }
}
