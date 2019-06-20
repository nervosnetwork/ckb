use super::super::block_verifier::{
    BlockBytesVerifier, BlockProposalsLimitVerifier, CellbaseVerifier, DuplicateVerifier,
    MerkleRootVerifier,
};
use super::super::error::{CellbaseError, Error as VerifyError};
use ckb_core::block::BlockBuilder;
use ckb_core::header::HeaderBuilder;
use ckb_core::script::Script;
use ckb_core::transaction::{
    CellInput, CellOutput, OutPoint, ProposalShortId, Transaction, TransactionBuilder,
};
use ckb_core::{capacity_bytes, BlockNumber, Bytes, Capacity};
use numext_fixed_hash::{h256, H256};

fn create_cellbase_transaction_with_block_number(number: BlockNumber) -> Transaction {
    TransactionBuilder::default()
        .input(CellInput::new_cellbase_input(number))
        .output(CellOutput::new(
            capacity_bytes!(100),
            Bytes::default(),
            Script::default(),
            None,
        ))
        .witness(Script::default().into_witness())
        .build()
}

fn create_cellbase_transaction_with_capacity(capacity: Capacity) -> Transaction {
    TransactionBuilder::default()
        .input(CellInput::new_cellbase_input(0))
        .output(CellOutput::new(
            capacity,
            Bytes::default(),
            Script::default(),
            None,
        ))
        .witness(Script::default().into_witness())
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
    let block = BlockBuilder::from_header_builder(HeaderBuilder::default().number(1))
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

    let block = BlockBuilder::from_header_builder(HeaderBuilder::default().number(1))
        .transaction(create_cellbase_transaction_with_block_number(1))
        .transaction(transaction)
        .build();

    let verifier = CellbaseVerifier::new();
    assert!(verifier.verify(&block).is_ok());
}

#[test]
pub fn test_block_with_correct_cellbase_number() {
    let block = BlockBuilder::from_header_builder(HeaderBuilder::default().number(2))
        .transaction(create_cellbase_transaction_with_block_number(2))
        .build();

    let verifier = CellbaseVerifier::new();
    assert!(verifier.verify(&block).is_ok());
}

#[test]
pub fn test_block_with_incorrect_cellbase_number() {
    let block = BlockBuilder::from_header_builder(HeaderBuilder::default().number(2))
        .transaction(create_cellbase_transaction_with_block_number(3))
        .build();

    let verifier = CellbaseVerifier::new();
    assert_eq!(
        verifier.verify(&block),
        Err(VerifyError::Cellbase(CellbaseError::InvalidInput))
    );
}

#[test]
pub fn test_block_with_one_cellbase_at_last() {
    let block = BlockBuilder::from_header_builder(HeaderBuilder::default().number(2))
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
pub fn test_block_with_duplicated_txs() {
    let tx = create_normal_transaction();
    let block = BlockBuilder::from_header_builder(HeaderBuilder::default().number(2))
        .transaction(tx.clone())
        .transaction(tx)
        .build();

    let verifier = DuplicateVerifier::new();
    assert_eq!(
        verifier.verify(&block),
        Err(VerifyError::CommitTransactionDuplicate)
    );
}

#[test]
pub fn test_block_with_duplicated_proposals() {
    let block = BlockBuilder::from_header_builder(HeaderBuilder::default().number(2))
        .proposal(ProposalShortId::zero())
        .proposal(ProposalShortId::zero())
        .build();

    let verifier = DuplicateVerifier::new();
    assert_eq!(
        verifier.verify(&block),
        Err(VerifyError::ProposalTransactionDuplicate)
    );
}

#[test]
pub fn test_transaction_root() {
    let header = HeaderBuilder::default()
        .number(2)
        .transactions_root(H256::zero());
    let block = unsafe {
        BlockBuilder::from_header_builder(header)
            .transaction(create_normal_transaction())
            .build_unchecked()
    };

    let verifier = MerkleRootVerifier::new();
    assert_eq!(
        verifier.verify(&block),
        Err(VerifyError::CommitTransactionsRoot)
    );
}

#[test]
pub fn test_proposals_root() {
    let header = HeaderBuilder::default()
        .number(2)
        .proposals_hash(h256!("0x1"));
    let block = unsafe {
        BlockBuilder::from_header_builder(header)
            .transaction(create_normal_transaction())
            .build_unchecked()
    };

    let verifier = MerkleRootVerifier::new();
    assert_eq!(
        verifier.verify(&block),
        Err(VerifyError::CommitTransactionsRoot)
    );
}

#[test]
pub fn test_witnesses_root() {
    let header = HeaderBuilder::default()
        .number(2)
        .witnesses_root(h256!("0x1"));
    let block = unsafe {
        BlockBuilder::from_header_builder(header)
            .proposal(ProposalShortId::zero())
            .build_unchecked()
    };

    let verifier = MerkleRootVerifier::new();
    assert_eq!(
        verifier.verify(&block),
        Err(VerifyError::WitnessesMerkleRoot)
    );
}

#[test]
pub fn test_block_with_two_cellbases() {
    let block = BlockBuilder::from_header_builder(HeaderBuilder::default().number(2))
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
