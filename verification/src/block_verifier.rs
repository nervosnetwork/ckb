use crate::error::{CellbaseError, Error};
use crate::header_verifier::HeaderResolver;
use crate::Verifier;
use ckb_chain_spec::consensus::Consensus;
use ckb_store::ChainStore;
use ckb_traits::ChainProvider;
use ckb_types::{
    core::{BlockView, EpochExt, HeaderView},
    packed::{CellInput, Script},
};
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
    type Target = BlockView;

    fn verify(&self, target: &BlockView) -> Result<(), Error> {
        let consensus = self.provider.consensus();
        let max_block_proposals_limit = consensus.max_block_proposals_limit();
        let max_block_bytes = consensus.max_block_bytes();
        BlockProposalsLimitVerifier::new(max_block_proposals_limit).verify(target)?;
        BlockBytesVerifier::new(max_block_bytes).verify(target)?;
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

    pub fn verify(&self, block: &BlockView) -> Result<(), Error> {
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
            return Err(Error::Cellbase(CellbaseError::InvalidQuantity));
        }

        let cellbase_transaction = &block.transactions()[0];

        if !cellbase_transaction.is_cellbase() {
            return Err(Error::Cellbase(CellbaseError::InvalidPosition));
        }

        if cellbase_transaction
            .witnesses()
            .get(0)
            .and_then(Script::from_witness)
            .is_none()
        {
            return Err(Error::Cellbase(CellbaseError::InvalidWitness));
        }

        if cellbase_transaction
            .outputs()
            .into_iter()
            .any(|output| output.type_().is_some())
        {
            return Err(Error::Cellbase(CellbaseError::InvalidTypeScript));
        }

        let cellbase_input = &cellbase_transaction
            .inputs()
            .get(0)
            .expect("cellbase should have input");
        if cellbase_input != &CellInput::new_cellbase_input(block.header().number()) {
            return Err(Error::Cellbase(CellbaseError::InvalidInput));
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

    pub fn verify(&self, block: &BlockView) -> Result<(), Error> {
        let mut seen = HashSet::with_capacity(block.transactions().len());
        if !block.transactions().iter().all(|tx| seen.insert(tx.hash())) {
            return Err(Error::CommitTransactionDuplicate);
        }

        let mut seen = HashSet::with_capacity(block.data().proposals().len());
        if !block
            .data()
            .proposals()
            .into_iter()
            .all(|id| seen.insert(id))
        {
            return Err(Error::ProposalTransactionDuplicate);
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

    pub fn verify(&self, block: &BlockView) -> Result<(), Error> {
        if block.transactions_root() != block.calc_transactions_root() {
            return Err(Error::CommitTransactionsRoot);
        }

        if block.witnesses_root() != block.calc_witnesses_root() {
            return Err(Error::WitnessesMerkleRoot);
        }

        if block.proposals_hash() != block.calc_proposals_hash() {
            return Err(Error::ProposalTransactionsRoot);
        }

        Ok(())
    }
}

pub struct HeaderResolverWrapper<'a> {
    header: &'a HeaderView,
    parent: Option<HeaderView>,
    epoch: Option<EpochExt>,
}

impl<'a> HeaderResolverWrapper<'a> {
    pub fn new<CS>(header: &'a HeaderView, store: &'a CS, consensus: &'a Consensus) -> Self
    where
        CS: ChainStore<'a>,
    {
        let parent = store.get_block_header(&header.data().raw().parent_hash());
        let epoch = parent
            .as_ref()
            .and_then(|parent| {
                store
                    .get_block_epoch(&parent.hash())
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

    pub fn build(
        header: &'a HeaderView,
        parent: Option<HeaderView>,
        epoch: Option<EpochExt>,
    ) -> Self {
        HeaderResolverWrapper {
            parent,
            header,
            epoch,
        }
    }
}

impl<'a> HeaderResolver for HeaderResolverWrapper<'a> {
    fn header(&self) -> &HeaderView {
        self.header
    }

    fn parent(&self) -> Option<&HeaderView> {
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

    pub fn verify(&self, block: &BlockView) -> Result<(), Error> {
        let proposals_len = block.data().proposals().len() as u64;
        if proposals_len <= self.block_proposals_limit {
            Ok(())
        } else {
            Err(Error::ExceededMaximumProposalsLimit)
        }
    }
}

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
        let block_bytes = block.serialized_size() as u64;
        if block_bytes <= self.block_bytes_limit {
            Ok(())
        } else {
            Err(Error::ExceededMaximumBlockBytes)
        }
    }
}
