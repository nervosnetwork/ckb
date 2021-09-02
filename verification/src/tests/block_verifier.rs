use super::super::block_verifier::{
    BlockBytesVerifier, BlockExtensionVerifier, BlockProposalsLimitVerifier, CellbaseVerifier,
    DuplicateVerifier, MerkleRootVerifier,
};
use crate::{BlockErrorKind, CellbaseError};
use ckb_chain_spec::consensus::ConsensusBuilder;
use ckb_error::assert_error_eq;
use ckb_types::{
    bytes::Bytes,
    core::{
        capacity_bytes, hardfork::HardForkSwitch, BlockBuilder, BlockNumber, Capacity,
        EpochNumberWithFraction, HeaderBuilder, TransactionBuilder, TransactionView,
    },
    h256,
    packed::{Byte32, CellInput, CellOutputBuilder, OutPoint, ProposalShortId, Script},
    prelude::*,
};

fn create_cellbase_transaction_with_block_number(number: BlockNumber) -> TransactionView {
    TransactionBuilder::default()
        .input(CellInput::new_cellbase_input(number))
        .output(
            CellOutputBuilder::default()
                .capacity(capacity_bytes!(100).pack())
                .build(),
        )
        .output_data(Bytes::new().pack())
        .witness(Script::default().into_witness())
        .build()
}

fn create_cellbase_transaction_with_capacity(capacity: Capacity) -> TransactionView {
    TransactionBuilder::default()
        .input(CellInput::new_cellbase_input(0))
        .output(
            CellOutputBuilder::default()
                .capacity(capacity.pack())
                .build(),
        )
        .output_data(Bytes::new().pack())
        .witness(Script::default().into_witness())
        .build()
}

fn create_cellbase_transaction_with_non_empty_output_data() -> TransactionView {
    TransactionBuilder::default()
        .input(CellInput::new_cellbase_input(0))
        .output(
            CellOutputBuilder::default()
                .capacity(capacity_bytes!(100).pack())
                .build(),
        )
        .output_data(Bytes::from("123").pack())
        .witness(Script::default().into_witness())
        .build()
}

fn create_cellbase_transaction_with_two_output() -> TransactionView {
    TransactionBuilder::default()
        .input(CellInput::new_cellbase_input(0))
        .output(
            CellOutputBuilder::default()
                .capacity(capacity_bytes!(100).pack())
                .build(),
        )
        .output(
            CellOutputBuilder::default()
                .capacity(capacity_bytes!(100).pack())
                .build(),
        )
        .output_data(Bytes::new().pack())
        .witness(Script::default().into_witness())
        .build()
}

fn create_cellbase_transaction_with_two_output_data() -> TransactionView {
    TransactionBuilder::default()
        .input(CellInput::new_cellbase_input(0))
        .output(
            CellOutputBuilder::default()
                .capacity(capacity_bytes!(100).pack())
                .build(),
        )
        .output_data(Bytes::new().pack())
        .output_data(Bytes::new().pack())
        .witness(Script::default().into_witness())
        .build()
}

fn create_cellbase_transaction() -> TransactionView {
    create_cellbase_transaction_with_capacity(capacity_bytes!(100))
}

fn create_normal_transaction() -> TransactionView {
    TransactionBuilder::default()
        .input(CellInput::new(OutPoint::new(h256!("0x1").pack(), 0), 0))
        .output(
            CellOutputBuilder::default()
                .capacity(capacity_bytes!(100).pack())
                .build(),
        )
        .output_data(Bytes::new().pack())
        .build()
}

#[test]
pub fn test_block_without_cellbase() {
    let block = BlockBuilder::default()
        .header(HeaderBuilder::default().number(1u64.pack()).build())
        .transaction(TransactionBuilder::default().build())
        .build();
    let verifier = CellbaseVerifier::new();
    assert_error_eq!(
        verifier.verify(&block).unwrap_err(),
        CellbaseError::InvalidQuantity,
    );
}

#[test]
pub fn test_block_with_one_cellbase_at_first() {
    let transaction = create_normal_transaction();

    let block = BlockBuilder::default()
        .header(HeaderBuilder::default().number(1u64.pack()).build())
        .transaction(create_cellbase_transaction_with_block_number(1))
        .transaction(transaction)
        .build();

    let verifier = CellbaseVerifier::new();
    assert!(verifier.verify(&block).is_ok());
}

#[test]
pub fn test_block_with_correct_cellbase_number() {
    let block = BlockBuilder::default()
        .header(HeaderBuilder::default().number(2u64.pack()).build())
        .transaction(create_cellbase_transaction_with_block_number(2))
        .build();

    let verifier = CellbaseVerifier::new();
    assert!(verifier.verify(&block).is_ok());
}

#[test]
pub fn test_block_with_incorrect_cellbase_number() {
    let block = BlockBuilder::default()
        .header(HeaderBuilder::default().number(2u64.pack()).build())
        .transaction(create_cellbase_transaction_with_block_number(3))
        .build();

    let verifier = CellbaseVerifier::new();
    assert_error_eq!(
        verifier.verify(&block).unwrap_err(),
        CellbaseError::InvalidInput,
    );
}

#[test]
pub fn test_block_with_one_cellbase_at_last() {
    let block = BlockBuilder::default()
        .header(HeaderBuilder::default().number(2u64.pack()).build())
        .transaction(create_normal_transaction())
        .transaction(create_cellbase_transaction())
        .build();

    let verifier = CellbaseVerifier::new();
    assert_error_eq!(
        verifier.verify(&block).unwrap_err(),
        CellbaseError::InvalidPosition,
    );
}

#[test]
pub fn test_cellbase_with_non_empty_output_data() {
    let block = BlockBuilder::default()
        .header(HeaderBuilder::default().number(2u64.pack()).build())
        .transaction(create_cellbase_transaction_with_non_empty_output_data())
        .build();
    let verifier = CellbaseVerifier::new();
    assert_error_eq!(
        verifier.verify(&block).unwrap_err(),
        CellbaseError::InvalidOutputData,
    );
}

#[test]
pub fn test_cellbase_without_output() {
    // without_output
    let cellbase_without_output = TransactionBuilder::default()
        .input(CellInput::new_cellbase_input(2u64))
        .witness(Script::default().into_witness())
        .build();
    let block = BlockBuilder::default()
        .header(HeaderBuilder::default().number(2u64.pack()).build())
        .transaction(cellbase_without_output)
        .build();
    let result = CellbaseVerifier::new().verify(&block);
    assert!(result.is_ok(), "Unexpected error {:?}", result);

    // only output_data
    let cellbase_without_output = TransactionBuilder::default()
        .input(CellInput::new_cellbase_input(2u64))
        .witness(Script::default().into_witness())
        .output_data(Bytes::new().pack())
        .build();
    let block = BlockBuilder::default()
        .header(HeaderBuilder::default().number(2u64.pack()).build())
        .transaction(cellbase_without_output)
        .build();
    let result = CellbaseVerifier::new().verify(&block);
    assert_error_eq!(result.unwrap_err(), CellbaseError::InvalidOutputQuantity);

    // only output
    let cellbase_without_output = TransactionBuilder::default()
        .input(CellInput::new_cellbase_input(2u64))
        .witness(Script::default().into_witness())
        .output(
            CellOutputBuilder::default()
                .capacity(capacity_bytes!(100).pack())
                .build(),
        )
        .build();
    let block = BlockBuilder::default()
        .header(HeaderBuilder::default().number(2u64.pack()).build())
        .transaction(cellbase_without_output)
        .build();
    let result = CellbaseVerifier::new().verify(&block);
    assert_error_eq!(result.unwrap_err(), CellbaseError::InvalidOutputQuantity);
}

#[test]
pub fn test_cellbase_with_two_output() {
    let block = BlockBuilder::default()
        .header(HeaderBuilder::default().number(2u64.pack()).build())
        .transaction(create_cellbase_transaction_with_two_output())
        .build();
    let verifier = CellbaseVerifier::new();
    assert_error_eq!(
        verifier.verify(&block).unwrap_err(),
        CellbaseError::InvalidOutputQuantity,
    )
}

#[test]
pub fn test_cellbase_with_two_output_data() {
    let block = BlockBuilder::default()
        .header(HeaderBuilder::default().number(2u64.pack()).build())
        .transaction(create_cellbase_transaction_with_two_output_data())
        .build();
    let verifier = CellbaseVerifier::new();
    assert_error_eq!(
        verifier.verify(&block).unwrap_err(),
        CellbaseError::InvalidOutputQuantity,
    )
}

#[test]
pub fn test_block_with_duplicated_txs() {
    let tx = create_normal_transaction();
    let block = BlockBuilder::default()
        .header(HeaderBuilder::default().number(2u64.pack()).build())
        .transaction(tx.clone())
        .transaction(tx)
        .build();

    let verifier = DuplicateVerifier::new();
    assert_error_eq!(
        verifier.verify(&block).unwrap_err(),
        BlockErrorKind::CommitTransactionDuplicate,
    );
}

#[test]
pub fn test_block_with_duplicated_proposals() {
    let block = BlockBuilder::default()
        .header(HeaderBuilder::default().number(2u64.pack()).build())
        .proposal(ProposalShortId::zero())
        .proposal(ProposalShortId::zero())
        .build();

    let verifier = DuplicateVerifier::new();
    assert_error_eq!(
        verifier.verify(&block).unwrap_err(),
        BlockErrorKind::ProposalTransactionDuplicate,
    );
}

#[test]
pub fn test_transaction_root() {
    let header = HeaderBuilder::default()
        .number(2u64.pack())
        .transactions_root(Byte32::zero())
        .build();
    let block = BlockBuilder::default()
        .header(header)
        .transaction(create_normal_transaction())
        .build_unchecked();

    let verifier = MerkleRootVerifier::new();
    assert_error_eq!(
        verifier.verify(&block).unwrap_err(),
        BlockErrorKind::TransactionsRoot,
    );
}

#[test]
pub fn test_proposals_root() {
    let header = HeaderBuilder::default()
        .number(2u64.pack())
        .proposals_hash(h256!("0x1").pack())
        .build();
    let block = BlockBuilder::default()
        .header(header)
        .transaction(create_normal_transaction())
        .build_unchecked();

    let verifier = MerkleRootVerifier::new();
    assert_error_eq!(
        verifier.verify(&block).unwrap_err(),
        BlockErrorKind::TransactionsRoot,
    );
}

#[test]
pub fn test_block_with_two_cellbases() {
    let block = BlockBuilder::default()
        .header(HeaderBuilder::default().number(2u64.pack()).build())
        .transaction(create_cellbase_transaction())
        .transaction(create_cellbase_transaction())
        .build();

    let verifier = CellbaseVerifier::new();
    assert_error_eq!(
        verifier.verify(&block).unwrap_err(),
        CellbaseError::InvalidQuantity,
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
    {
        let verifier =
            BlockBytesVerifier::new(block.data().serialized_size_without_uncle_proposals() as u64);
        assert!(verifier.verify(&block).is_ok());
    }

    {
        let verifier = BlockBytesVerifier::new(
            block.data().serialized_size_without_uncle_proposals() as u64 - 1,
        );
        assert!(verifier.verify(&block).is_ok());
    }
}

#[test]
pub fn test_max_block_bytes_verifier() {
    let block = BlockBuilder::default()
        .header(HeaderBuilder::default().number(2u64.pack()).build())
        .build();

    {
        let verifier =
            BlockBytesVerifier::new(block.data().serialized_size_without_uncle_proposals() as u64);
        assert!(verifier.verify(&block).is_ok());
    }

    {
        let verifier = BlockBytesVerifier::new(
            block.data().serialized_size_without_uncle_proposals() as u64 - 1,
        );
        assert_error_eq!(
            verifier.verify(&block).unwrap_err(),
            BlockErrorKind::ExceededMaximumBlockBytes,
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
        assert!(verifier.verify(&block).is_ok());
    }

    {
        let verifier = BlockProposalsLimitVerifier::new(0);
        assert_error_eq!(
            verifier.verify(&block).unwrap_err(),
            BlockErrorKind::ExceededMaximumProposalsLimit,
        );
    }
}

#[test]
fn test_block_extension_verifier() {
    let fork_at = 10;
    let epoch = EpochNumberWithFraction::new(fork_at, 0, 1);

    // normal block (no uncles)
    let header = HeaderBuilder::default().epoch(epoch.pack()).build();
    let block = BlockBuilder::default().header(header).build();

    // invalid extra hash (no extension)
    let header1 = block
        .header()
        .as_advanced_builder()
        .extra_hash(h256!("0x1").pack())
        .build();
    let block1 = BlockBuilder::default().header(header1).build_unchecked();

    // empty extension
    let block2 = block
        .as_advanced_builder()
        .extension(Some(Default::default()))
        .build();
    // extension has only 1 byte
    let block3 = block
        .as_advanced_builder()
        .extension(Some(vec![0u8].pack()))
        .build();
    // extension has 96 bytes
    let block4 = block
        .as_advanced_builder()
        .extension(Some(vec![0u8; 96].pack()))
        .build();
    // extension has 97 bytes
    let block5 = block
        .as_advanced_builder()
        .extension(Some(vec![0u8; 97].pack()))
        .build();

    // normal block (with uncles)
    let block6 = block
        .as_advanced_builder()
        .uncle(BlockBuilder::default().build().as_uncle())
        .build();

    // invalid extra hash (has extension but use uncles hash)
    let block7 = block6
        .as_advanced_builder()
        .extension(Some(vec![0u8; 32].pack()))
        .build_unchecked();

    {
        // Test CKB v2019
        let hardfork_switch = HardForkSwitch::new_without_any_enabled()
            .as_builder()
            .rfc_0031(fork_at + 1)
            .build()
            .unwrap();
        let consensus = ConsensusBuilder::default()
            .hardfork_switch(hardfork_switch)
            .build();

        let result = BlockExtensionVerifier::new(&consensus).verify(&block);
        assert!(result.is_ok(), "result = {:?}", result);

        let result = BlockExtensionVerifier::new(&consensus).verify(&block1);
        assert_error_eq!(result.unwrap_err(), BlockErrorKind::InvalidExtraHash);

        let result = BlockExtensionVerifier::new(&consensus).verify(&block2);
        assert_error_eq!(result.unwrap_err(), BlockErrorKind::UnknownFields);

        let result = BlockExtensionVerifier::new(&consensus).verify(&block3);
        assert_error_eq!(result.unwrap_err(), BlockErrorKind::UnknownFields);

        let result = BlockExtensionVerifier::new(&consensus).verify(&block4);
        assert_error_eq!(result.unwrap_err(), BlockErrorKind::UnknownFields);

        let result = BlockExtensionVerifier::new(&consensus).verify(&block5);
        assert_error_eq!(result.unwrap_err(), BlockErrorKind::UnknownFields);

        let result = BlockExtensionVerifier::new(&consensus).verify(&block6);
        assert!(result.is_ok(), "result = {:?}", result);

        let result = BlockExtensionVerifier::new(&consensus).verify(&block7);
        assert_error_eq!(result.unwrap_err(), BlockErrorKind::UnknownFields);
    }
    {
        // Test CKB v2021
        let hardfork_switch = HardForkSwitch::new_without_any_enabled()
            .as_builder()
            .rfc_0031(fork_at)
            .build()
            .unwrap();
        let consensus = ConsensusBuilder::default()
            .hardfork_switch(hardfork_switch)
            .build();

        let result = BlockExtensionVerifier::new(&consensus).verify(&block);
        assert!(result.is_ok(), "result = {:?}", result);

        let result = BlockExtensionVerifier::new(&consensus).verify(&block1);
        assert_error_eq!(result.unwrap_err(), BlockErrorKind::InvalidExtraHash);

        let result = BlockExtensionVerifier::new(&consensus).verify(&block2);
        assert_error_eq!(result.unwrap_err(), BlockErrorKind::EmptyBlockExtension);

        let result = BlockExtensionVerifier::new(&consensus).verify(&block3);
        assert!(result.is_ok(), "result = {:?}", result);

        let result = BlockExtensionVerifier::new(&consensus).verify(&block4);
        assert!(result.is_ok(), "result = {:?}", result);

        let result = BlockExtensionVerifier::new(&consensus).verify(&block5);
        assert_error_eq!(
            result.unwrap_err(),
            BlockErrorKind::ExceededMaximumBlockExtensionBytes
        );

        let result = BlockExtensionVerifier::new(&consensus).verify(&block6);
        assert!(result.is_ok(), "result = {:?}", result);

        let result = BlockExtensionVerifier::new(&consensus).verify(&block7);
        assert_error_eq!(result.unwrap_err(), BlockErrorKind::InvalidExtraHash);
    }
}
