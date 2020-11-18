use super::Verifier;
use crate::{
    error::CellbaseError, BlockErrorKind, BlockVerifier, EpochError, NumberError, UnclesError,
    UnknownParentError,
};
use ckb_chain_spec::{calculate_block_reward, consensus::Consensus};
use ckb_dao_utils::genesis_dao_data_with_satoshi_gift;
use ckb_error::Error;
use ckb_types::{core::BlockView, packed::CellInput};

/// TODO(doc): @zhangsoledad
#[derive(Clone)]
pub struct GenesisVerifier {}

impl GenesisVerifier {
    /// TODO(doc): @zhangsoledad
    pub fn new() -> Self {
        GenesisVerifier {}
    }
}

impl Default for GenesisVerifier {
    fn default() -> Self {
        Self::new()
    }
}

impl Verifier for GenesisVerifier {
    type Target = Consensus;

    fn verify(&self, consensus: &Self::Target) -> Result<(), Error> {
        NumberVerifier::verify(consensus.genesis_block())?;
        EpochVerifier::verify(consensus.genesis_block())?;
        ParentHashVerifier::verify(consensus.genesis_block())?;
        CellbaseVerifier::verify(consensus.genesis_block())?;
        UnclesVerifier::verify(consensus.genesis_block())?;
        DAOVerifier::new(consensus).verify(consensus.genesis_block())?;
        BlockVerifier::new(consensus).verify(consensus.genesis_block())
    }
}

#[derive(Clone)]
pub struct NumberVerifier {}

impl NumberVerifier {
    pub fn verify(block: &BlockView) -> Result<(), Error> {
        if block.header().number() != 0 {
            return Err((NumberError {
                expected: 0,
                actual: block.header().number(),
            })
            .into());
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct EpochVerifier {}

impl EpochVerifier {
    pub fn verify(block: &BlockView) -> Result<(), Error> {
        if block.header().epoch().number() != 0 {
            return Err((EpochError::NumberMismatch {
                expected: 0,
                actual: block.header().epoch().number(),
            })
            .into());
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct ParentHashVerifier {}

impl ParentHashVerifier {
    pub fn verify(block: &BlockView) -> Result<(), Error> {
        if block.parent_hash().raw_data()[..] != [0u8; 32][..] {
            return Err((UnknownParentError {
                parent_hash: block.parent_hash(),
            })
            .into());
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct UnclesVerifier {}

impl UnclesVerifier {
    pub fn verify(block: &BlockView) -> Result<(), Error> {
        if !block.uncles().hashes().is_empty() {
            return Err((UnclesError::OverCount {
                max: 0,
                actual: block.uncles().hashes().len() as u32,
            })
            .into());
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct DAOVerifier<'a> {
    consensus: &'a Consensus,
}

impl<'a> DAOVerifier<'a> {
    pub fn new(consensus: &'a Consensus) -> Self {
        DAOVerifier { consensus }
    }

    pub fn verify(&self, block: &BlockView) -> Result<(), Error> {
        let txs = block.transactions();
        let epoch_length = self.consensus.genesis_epoch_ext.length();
        let primary_issuance =
            calculate_block_reward(self.consensus.initial_primary_epoch_reward, epoch_length);
        let secondary_issuance =
            calculate_block_reward(self.consensus.secondary_epoch_reward, epoch_length);
        let dao = genesis_dao_data_with_satoshi_gift(
            txs.iter().collect::<Vec<_>>(),
            &self.consensus.satoshi_pubkey_hash,
            self.consensus.satoshi_cell_occupied_ratio,
            primary_issuance,
            secondary_issuance,
        )?;
        if dao != block.header().dao() {
            return Err((BlockErrorKind::InvalidDAO).into());
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct CellbaseVerifier {}

impl CellbaseVerifier {
    pub fn verify(block: &BlockView) -> Result<(), Error> {
        let cellbase_len = block
            .transactions()
            .iter()
            .filter(|tx| tx.is_cellbase())
            .count();

        // empty checked, block must contain cellbase
        if cellbase_len != 1 {
            return Err((CellbaseError::InvalidQuantity).into());
        }

        let cellbase_transaction = &block.transactions()[0];

        if !cellbase_transaction.is_cellbase() {
            return Err((CellbaseError::InvalidPosition).into());
        }

        // cellbase outputs/outputs_data len must be equalized
        if cellbase_transaction.outputs().len() != cellbase_transaction.outputs_data().len() {
            return Err((CellbaseError::InvalidOutputQuantity).into());
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
