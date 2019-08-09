use crate::header_verifier::HeaderResolver;
use crate::Verifier;
use ckb_chain_spec::consensus::Consensus;
use ckb_core::block::Block;
use ckb_core::extras::EpochExt;
use ckb_core::header::Header;
use ckb_core::script::Script;
use ckb_core::transaction::CellInput;
use ckb_error::{BlockError, CellbaseError, Error};
use ckb_store::ChainStore;
use ckb_traits::ChainProvider;
use std::collections::HashSet;

//TODO: cellbase, witness
#[derive(Clone)]
pub struct BlockVerifier<P> {
    provider: P,
}

impl<P> BlockVerifier<P>
where
    P: ChainProvider + Clone,
{
    pub fn new(provider: P) -> Self {
        BlockVerifier { provider }
    }
}

impl<P> Verifier for BlockVerifier<P>
where
    P: ChainProvider + Clone,
{
    type Target = Block;

    fn verify(&self, target: &Block) -> Result<(), Error> {
        let consensus = self.provider.consensus();
        let proof_size = consensus.pow_engine().proof_size();
        let max_block_proposals_limit = consensus.max_block_proposals_limit();
        let max_block_bytes = consensus.max_block_bytes();
        BlockProposalsLimitVerifier::new(max_block_proposals_limit).verify(target)?;
        BlockBytesVerifier::new(max_block_bytes, proof_size).verify(target)?;
        CellbaseVerifier::new().verify(target)?;
        DuplicateVerifier::new().verify(target)?;
        MerkleRootVerifier::new().verify(target)
    }
}

#[derive(Clone)]
pub struct CellbaseVerifier {}

impl CellbaseVerifier {
    pub fn new() -> Self {
        CellbaseVerifier {}
    }

    pub fn verify(&self, block: &Block) -> Result<(), Error> {
        if block.is_genesis() {
            return Ok(());
        }

        let cellbase_len = block
            .transactions()
            .iter()
            .filter(|tx| tx.is_cellbase())
            .count();

        // empty checked, block must contain cellbase
        if cellbase_len != 1 {
            Err(BlockError::Cellbase(CellbaseError::InvalidQuantity))?;
        }

        let cellbase_transaction = &block.transactions()[0];

        if !cellbase_transaction.is_cellbase() {
            Err(BlockError::Cellbase(CellbaseError::InvalidPosition))?;
        }

        if cellbase_transaction
            .witnesses()
            .get(0)
            .and_then(|witness| Script::from_witness(witness))
            .is_none()
        {
            Err(BlockError::Cellbase(CellbaseError::InvalidWitness))?;
        }

        if cellbase_transaction
            .outputs()
            .iter()
            .any(|output| output.type_.is_some())
        {
            Err(BlockError::Cellbase(CellbaseError::InvalidTypeScript))?;
        }

        let cellbase_input = &cellbase_transaction.inputs()[0];
        if cellbase_input != &CellInput::new_cellbase_input(block.header().number()) {
            Err(BlockError::Cellbase(CellbaseError::InvalidInput))?;
        }

        Ok(())
    }
}

#[derive(Clone)]
pub struct DuplicateVerifier {}

impl DuplicateVerifier {
    pub fn new() -> Self {
        DuplicateVerifier {}
    }

    pub fn verify(&self, block: &Block) -> Result<(), Error> {
        let mut seen = HashSet::with_capacity(block.transactions().len());
        if !block.transactions().iter().all(|tx| seen.insert(tx.hash())) {
            Err(BlockError::DuplicatedCommittedTransactions)?;
        }

        let mut seen = HashSet::with_capacity(block.proposals().len());
        if !block.proposals().iter().all(|id| seen.insert(id)) {
            Err(BlockError::DuplicatedProposalTransactions)?;
        }
        Ok(())
    }
}

#[derive(Clone, Default)]
pub struct MerkleRootVerifier {}

impl MerkleRootVerifier {
    pub fn new() -> Self {
        MerkleRootVerifier::default()
    }

    pub fn verify(&self, block: &Block) -> Result<(), Error> {
        if block.header().transactions_root() != &block.cal_transactions_root() {
            Err(BlockError::UnmatchedCommittedRoot)?;
        }

        if block.header().witnesses_root() != &block.cal_witnesses_root() {
            Err(BlockError::UnmatchedWitnessesRoot)?;
        }

        if block.header().proposals_hash() != &block.cal_proposals_hash() {
            Err(BlockError::UnmatchedProposalRoot)?;
        }

        Ok(())
    }
}

pub struct HeaderResolverWrapper<'a> {
    header: &'a Header,
    parent: Option<Header>,
    epoch: Option<EpochExt>,
}

impl<'a> HeaderResolverWrapper<'a> {
    pub fn new<CS>(header: &'a Header, store: &'a CS, consensus: &'a Consensus) -> Self
    where
        CS: ChainStore<'a>,
    {
        let parent = store.get_block_header(header.parent_hash());
        let epoch = parent
            .as_ref()
            .and_then(|parent| {
                store
                    .get_block_epoch(parent.hash())
                    .map(|ext| (parent, ext))
            })
            .map(|(parent, last_epoch)| {
                store
                    .next_epoch_ext(consensus, &last_epoch, &parent)
                    .unwrap_or(last_epoch)
            });

        HeaderResolverWrapper {
            parent,
            header,
            epoch,
        }
    }

    pub fn build(header: &'a Header, parent: Option<Header>, epoch: Option<EpochExt>) -> Self {
        HeaderResolverWrapper {
            parent,
            header,
            epoch,
        }
    }
}

impl<'a> HeaderResolver for HeaderResolverWrapper<'a> {
    fn header(&self) -> &Header {
        self.header
    }

    fn parent(&self) -> Option<&Header> {
        self.parent.as_ref()
    }

    fn epoch(&self) -> Option<&EpochExt> {
        self.epoch.as_ref()
    }
}

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

    pub fn verify(&self, block: &Block) -> Result<(), Error> {
        let proposals_len = block.proposals().len() as u64;
        if proposals_len <= self.block_proposals_limit {
            Ok(())
        } else {
            Err(BlockError::TooManyProposals)?
        }
    }
}

#[derive(Clone)]
pub struct BlockBytesVerifier {
    block_bytes_limit: u64,
    proof_size: usize,
}

impl BlockBytesVerifier {
    pub fn new(block_bytes_limit: u64, proof_size: usize) -> Self {
        BlockBytesVerifier {
            block_bytes_limit,
            proof_size,
        }
    }

    pub fn verify(&self, block: &Block) -> Result<(), Error> {
        // Skip bytes limit on genesis block
        if block.is_genesis() {
            return Ok(());
        }
        let block_bytes = block.serialized_size(self.proof_size) as u64;
        if block_bytes <= self.block_bytes_limit {
            Ok(())
        } else {
            Err(BlockError::TooLargeSize)?
        }
    }
}
