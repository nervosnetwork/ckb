//! Transaction verification environment.

use ckb_chain_spec::consensus::ProposalWindow;
use ckb_types::{
    core::{BlockNumber, EpochNumber, EpochNumberWithFraction, HeaderView},
    packed::Byte32,
};

/// The phase that transactions are in.
#[derive(Debug, Clone, Copy)]
enum TxVerifyPhase {
    /// The transaction has just been submitted.
    ///
    /// So the transaction will be:
    /// - proposed after (or in) the `tip_number + 1` block.
    /// - committed after (or in) `tip_number + 1 + proposal_window.closest()` block.
    Submitted,
    /// The transaction has already been proposed before several blocks.
    ///
    /// Assume that the inner block number is `N`.
    /// So the transaction is proposed in the `tip_number - N` block.
    /// Then it will be committed after (or in) the `tip_number - N + proposal_window.closest()` block.
    Proposed(BlockNumber),
    /// The transaction is commit.
    ///
    /// So the transaction will be committed in current block.
    Committed,
}

/// The environment that transactions are in.
#[derive(Debug, Clone)]
pub struct TxVerifyEnv {
    // Please keep these fields to be private.
    // So we can update this struct easier when we want to add more data.
    phase: TxVerifyPhase,
    // Current Tip Environment
    number: BlockNumber,
    epoch: EpochNumberWithFraction,
    hash: Byte32,
    parent_hash: Byte32,
}

impl TxVerifyEnv {
    /// The transaction has just been submitted.
    ///
    /// The input is current tip header.
    pub fn new_submit(header: &HeaderView) -> Self {
        Self {
            phase: TxVerifyPhase::Submitted,
            number: header.number(),
            epoch: header.epoch(),
            hash: header.hash(),
            parent_hash: header.parent_hash(),
        }
    }

    /// The transaction has already been proposed before several blocks.
    ///
    /// The input is current tip header and how many blocks have been passed since the transaction was proposed.
    pub fn new_proposed(header: &HeaderView, n_blocks: BlockNumber) -> Self {
        Self {
            phase: TxVerifyPhase::Proposed(n_blocks),
            number: header.number(),
            epoch: header.epoch(),
            hash: header.hash(),
            parent_hash: header.parent_hash(),
        }
    }

    /// The transaction will committed in current block.
    ///
    /// The input is current tip header.
    pub fn new_commit(header: &HeaderView) -> Self {
        Self {
            phase: TxVerifyPhase::Committed,
            number: header.number(),
            epoch: header.epoch(),
            hash: header.hash(),
            parent_hash: header.parent_hash(),
        }
    }

    /// The block number of the earliest block which the transaction will committed in.
    pub fn block_number(&self, proposal_window: ProposalWindow) -> BlockNumber {
        match self.phase {
            TxVerifyPhase::Submitted => self.number + 1 + proposal_window.closest(),
            TxVerifyPhase::Proposed(already_proposed) => {
                self.number.saturating_sub(already_proposed) + proposal_window.closest()
            }
            TxVerifyPhase::Committed => self.number,
        }
    }

    /// The epoch number of the earliest epoch which the transaction will committed in.
    pub fn epoch_number(&self, proposal_window: ProposalWindow) -> EpochNumber {
        let n_blocks = match self.phase {
            TxVerifyPhase::Submitted => 1 + proposal_window.closest(),
            TxVerifyPhase::Proposed(already_proposed) => {
                proposal_window.closest().saturating_sub(already_proposed)
            }
            TxVerifyPhase::Committed => 0,
        };
        self.epoch.minimum_epoch_number_after_n_blocks(n_blocks)
    }

    /// The parent block hash of the earliest block which the transaction will committed in.
    pub fn parent_hash(&self) -> Byte32 {
        match self.phase {
            TxVerifyPhase::Submitted => &self.hash,
            TxVerifyPhase::Proposed(_) => &self.hash,
            TxVerifyPhase::Committed => &self.parent_hash,
        }
        .to_owned()
    }

    /// The earliest epoch which the transaction will committed in.
    pub fn epoch(&self) -> EpochNumberWithFraction {
        self.epoch
    }

    /// The epoch number of the earliest epoch which the transaction will committed in without
    /// consider about the proposal window.
    pub fn epoch_number_without_proposal_window(&self) -> EpochNumber {
        let n_blocks = match self.phase {
            TxVerifyPhase::Submitted | TxVerifyPhase::Proposed(_) => 1,
            TxVerifyPhase::Committed => 0,
        };
        self.epoch.minimum_epoch_number_after_n_blocks(n_blocks)
    }
}
