use crate::{
    BlockErrorKind, CellbaseError, transaction_verifier::NonContextualTransactionVerifier,
};
use ckb_chain_spec::consensus::Consensus;
use ckb_error::Error;
use ckb_types::{
    core::{BlockView, ScriptHashType},
    packed::{CellInput, CellbaseWitness},
    prelude::*,
};
use ckb_verification_traits::Verifier;
use std::collections::HashSet;

/// Block verifier that are independent of context.
///
/// Contains:
/// - [`CellbaseVerifier`](./struct.CellbaseVerifier.html)
/// - [`BlockBytesVerifier`](./struct.BlockBytesVerifier.html)
/// - [`BlockExtensionVerifier`](./struct.BlockExtensionVerifier.html)
/// - [`BlockProposalsLimitVerifier`](./struct.BlockProposalsLimitVerifier.html)
/// - [`DuplicateVerifier`](./struct.DuplicateVerifier.html)
/// - [`MerkleRootVerifier`](./struct.MerkleRootVerifier.html)
#[derive(Clone)]
pub struct BlockVerifier<'a> {
    consensus: &'a Consensus,
}

impl<'a> BlockVerifier<'a> {
    /// Constructs a BlockVerifier
    pub fn new(consensus: &'a Consensus) -> Self {
        BlockVerifier { consensus }
    }
}

impl<'a> Verifier for BlockVerifier<'a> {
    type Target = BlockView;

    fn verify(&self, target: &BlockView) -> Result<(), Error> {
        let max_block_proposals_limit = self.consensus.max_block_proposals_limit();
        let max_block_bytes = self.consensus.max_block_bytes();
        BlockProposalsLimitVerifier::new(max_block_proposals_limit).verify(target)?;
        BlockBytesVerifier::new(max_block_bytes).verify(target)?;
        CellbaseVerifier::new().verify(target)?;
        DuplicateVerifier::new().verify(target)?;
        MerkleRootVerifier::new().verify(target)
    }
}

/// Cellbase verifier
///
/// First transaction must be cellbase, the rest must not be.
/// Cellbase outputs/outputs_data len must le 1, and outputs len must equal to outputs_data len.
/// Cellbase output data must be empty
/// Cellbase output type_ must be empty
/// Cellbase has only one dummy input. The input's `since` field must be equal to the block number.
#[derive(Clone)]
pub struct CellbaseVerifier {}

impl CellbaseVerifier {
    /// Constructs a CellbaseVerifier
    pub fn new() -> Self {
        CellbaseVerifier {}
    }

    pub fn verify(&self, block: &BlockView) -> Result<(), Error> {
        if block.is_genesis() {
            return Ok(());
        }

        let cellbase_len = block
            .transactions()
            .iter()
            .filter(|tx| tx.is_cellbase())
            .count();

        // check cellbase count, block must contain ONLY one cellbase
        if cellbase_len != 1 {
            return Err((CellbaseError::InvalidQuantity).into());
        }

        let cellbase_transaction = &block.transactions()[0];

        if !cellbase_transaction.is_cellbase() {
            return Err((CellbaseError::InvalidPosition).into());
        }

        // cellbase outputs/outputs_data len must le 1, and outputs len must equal to outputs_data len
        if cellbase_transaction.outputs().len() > 1
            || cellbase_transaction.outputs_data().len() > 1
            || cellbase_transaction.outputs().len() != cellbase_transaction.outputs_data().len()
        {
            return Err((CellbaseError::InvalidOutputQuantity).into());
        }

        // cellbase output data must be empty
        if !cellbase_transaction
            .outputs_data()
            .get(0)
            .map(|data| data.is_empty())
            .unwrap_or(true)
        {
            return Err((CellbaseError::InvalidOutputData).into());
        }

        if cellbase_transaction
            .witnesses()
            .get(0)
            .and_then(|witness| {
                CellbaseWitness::from_slice(&witness.raw_data())
                    .ok()
                    .and_then(|cellbase_witness| {
                        ScriptHashType::try_from(cellbase_witness.lock().hash_type()).ok()
                    })
            })
            .is_none()
        {
            return Err((CellbaseError::InvalidWitness).into());
        }

        // cellbase output type_ must be empty
        if cellbase_transaction
            .outputs()
            .into_iter()
            .any(|output| output.type_().is_some())
        {
            return Err((CellbaseError::InvalidTypeScript).into());
        }

        let cellbase_input = &cellbase_transaction
            .inputs()
            .get(0)
            .expect("cellbase should have input");
        if cellbase_input != &CellInput::new_cellbase_input(block.header().number()) {
            return Err((CellbaseError::InvalidInput).into());
        }

        Ok(())
    }
}

/// DuplicateVerifier
///
/// Verifying that a block does not contain any duplicate transactions or
/// proposals.
#[derive(Clone)]
pub struct DuplicateVerifier {}

impl DuplicateVerifier {
    pub fn new() -> Self {
        DuplicateVerifier {}
    }

    pub fn verify(&self, block: &BlockView) -> Result<(), Error> {
        let mut seen = HashSet::with_capacity(block.transactions().len());
        if !block.transactions().iter().all(|tx| seen.insert(tx.hash())) {
            return Err((BlockErrorKind::CommitTransactionDuplicate).into());
        }

        let mut seen = HashSet::with_capacity(block.data().proposals().len());
        if !block
            .data()
            .proposals()
            .into_iter()
            .all(|id| seen.insert(id))
        {
            return Err((BlockErrorKind::ProposalTransactionDuplicate).into());
        }
        Ok(())
    }
}

/// MerkleRootVerifier
///
/// Check the merkle root
#[derive(Clone, Default)]
pub struct MerkleRootVerifier {}

impl MerkleRootVerifier {
    pub fn new() -> Self {
        MerkleRootVerifier::default()
    }

    pub fn verify(&self, block: &BlockView) -> Result<(), Error> {
        if block.transactions_root() != block.calc_transactions_root() {
            return Err(BlockErrorKind::TransactionsRoot.into());
        }

        if block.proposals_hash() != block.calc_proposals_hash() {
            return Err(BlockErrorKind::ProposalTransactionsHash.into());
        }

        Ok(())
    }
}

/// BlockProposalsLimitVerifier.
///
/// Check block proposal limit.
#[derive(Clone)]
pub struct BlockProposalsLimitVerifier {
    block_proposals_limit: u64,
}

impl BlockProposalsLimitVerifier {
    pub fn new(block_proposals_limit: u64) -> Self {
        BlockProposalsLimitVerifier {
            block_proposals_limit,
        }
    }

    pub fn verify(&self, block: &BlockView) -> Result<(), Error> {
        let proposals_len = block.data().proposals().len() as u64;
        if proposals_len <= self.block_proposals_limit {
            Ok(())
        } else {
            Err(BlockErrorKind::ExceededMaximumProposalsLimit.into())
        }
    }
}

/// BlockBytesVerifier.
///
/// Check block size limit.
#[derive(Clone)]
pub struct BlockBytesVerifier {
    block_bytes_limit: u64,
}

impl BlockBytesVerifier {
    pub fn new(block_bytes_limit: u64) -> Self {
        BlockBytesVerifier { block_bytes_limit }
    }

    pub fn verify(&self, block: &BlockView) -> Result<(), Error> {
        // Skip bytes limit on genesis block
        if block.is_genesis() {
            return Ok(());
        }
        let block_bytes = block.data().serialized_size_without_uncle_proposals() as u64;
        if block_bytes <= self.block_bytes_limit {
            Ok(())
        } else {
            Err(BlockErrorKind::ExceededMaximumBlockBytes.into())
        }
    }
}

/// Context-independent verification checks for block transactions
///
/// Basic checks that don't depend on any context
/// See [`NonContextualTransactionVerifier`](./struct.NonContextualBlockTxsVerifier.html)
pub struct NonContextualBlockTxsVerifier<'a> {
    consensus: &'a Consensus,
}

impl<'a> NonContextualBlockTxsVerifier<'a> {
    /// Creates a new NonContextualBlockTxsVerifier
    pub fn new(consensus: &'a Consensus) -> Self {
        NonContextualBlockTxsVerifier { consensus }
    }

    /// Perform context-independent verification checks for block transactions
    pub fn verify(&self, block: &BlockView) -> Result<Vec<()>, Error> {
        block
            .transactions()
            .iter()
            .map(|tx| NonContextualTransactionVerifier::new(tx, self.consensus).verify())
            .collect()
    }
}
