use crate::error::{CellbaseError, Error, UnclesError};
use crate::header_verifier::HeaderResolver;
use crate::Verifier;
use ckb_core::block::Block;
use ckb_core::extras::EpochExt;
use ckb_core::header::Header;
use ckb_core::transaction::CellInput;
use ckb_traits::ChainProvider;
use fnv::FnvHashSet;
use std::collections::HashSet;

#[derive(Clone)]
pub struct BlockVerifier<P> {
    provider: P,
}

fn prepare_epoch_ext<P: ChainProvider>(provider: &P, block: &Block) -> Result<EpochExt, Error> {
    if block.is_genesis() {
        return Ok(provider.consensus().genesis_epoch_ext().to_owned());
    }
    let parent_hash = block.header().parent_hash();
    let parent_ext = provider
        .get_block_epoch(parent_hash)
        .ok_or_else(|| Error::UnknownParent(parent_hash.clone()))?;
    let parent = provider
        .block_header(parent_hash)
        .ok_or_else(|| Error::UnknownParent(parent_hash.clone()))?;
    Ok(provider
        .next_epoch_ext(&parent_ext, &parent)
        .unwrap_or(parent_ext))
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
        let epoch_ext = prepare_epoch_ext(&self.provider, target)?;
        BlockProposalsLimitVerifier::new(max_block_proposals_limit).verify(target)?;
        BlockBytesVerifier::new(max_block_bytes, proof_size).verify(target)?;
        CellbaseVerifier::new().verify(target)?;
        DuplicateVerifier::new().verify(target)?;
        MerkleRootVerifier::new().verify(target)?;
        UnclesVerifier::new(self.provider.clone(), &epoch_ext).verify(target)
    }
}

#[derive(Clone)]
pub struct CellbaseVerifier {}

impl CellbaseVerifier {
    pub fn new() -> Self {
        CellbaseVerifier {}
    }

    pub fn verify(&self, block: &Block) -> Result<(), Error> {
        let cellbase_len = block
            .transactions()
            .iter()
            .filter(|tx| tx.is_cellbase())
            .count();

        // empty checked, block must contain cellbase
        if cellbase_len != 1 {
            return Err(Error::Cellbase(CellbaseError::InvalidQuantity));
        }

        if block.get_cellbase_lock().is_none() {
            return Err(Error::Cellbase(CellbaseError::InvalidOutput));
        }

        let cellbase_transaction = &block.transactions()[0];
        if !cellbase_transaction.is_cellbase() {
            return Err(Error::Cellbase(CellbaseError::InvalidPosition));
        }

        let cellbase_input = &cellbase_transaction.inputs()[0];
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

    pub fn verify(&self, block: &Block) -> Result<(), Error> {
        let mut seen = HashSet::with_capacity(block.transactions().len());
        if !block.transactions().iter().all(|tx| seen.insert(tx.hash())) {
            return Err(Error::CommitTransactionDuplicate);
        }

        let mut seen = HashSet::with_capacity(block.proposals().len());
        if !block.proposals().iter().all(|id| seen.insert(id)) {
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

    pub fn verify(&self, block: &Block) -> Result<(), Error> {
        if block.header().transactions_root() != &block.cal_transactions_root() {
            return Err(Error::CommitTransactionsRoot);
        }

        if block.header().witnesses_root() != &block.cal_witnesses_root() {
            return Err(Error::WitnessesMerkleRoot);
        }

        if block.header().proposals_hash() != &block.cal_proposals_hash() {
            return Err(Error::ProposalTransactionsRoot);
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
    pub fn new<CP>(header: &'a Header, provider: CP) -> Self
    where
        CP: ChainProvider,
    {
        let parent = provider.block_header(&header.parent_hash());
        let epoch = parent
            .as_ref()
            .and_then(|parent| {
                provider
                    .get_block_epoch(&parent.hash())
                    .map(|ext| (parent, ext))
            })
            .map(|(parent, last_epoch)| {
                provider
                    .next_epoch_ext(&last_epoch, parent)
                    .unwrap_or(last_epoch)
            });

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

// TODO redo uncle verifier, check uncle proposal duplicate
#[derive(Clone)]
pub struct UnclesVerifier<'a, P> {
    provider: P,
    epoch: &'a EpochExt,
}

impl<'a, P> UnclesVerifier<'a, P>
where
    P: ChainProvider + Clone,
{
    pub fn new(provider: P, epoch: &'a EpochExt) -> Self {
        UnclesVerifier { provider, epoch }
    }

    // -  uncles_hash
    // -  uncles_num
    // -  depth
    // -  uncle not in main chain
    // -  uncle duplicate
    pub fn verify(&self, block: &Block) -> Result<(), Error> {
        // verify uncles_count
        let uncles_count = block.uncles().len() as u32;
        if uncles_count != block.header().uncles_count() {
            return Err(Error::Uncles(UnclesError::MissMatchCount {
                expected: block.header().uncles_count(),
                actual: uncles_count,
            }));
        }

        // verify uncles_hash
        let actual_uncles_hash = block.cal_uncles_hash();
        if &actual_uncles_hash != block.header().uncles_hash() {
            return Err(Error::Uncles(UnclesError::InvalidHash {
                expected: block.header().uncles_hash().to_owned(),
                actual: actual_uncles_hash,
            }));
        }

        // if block.uncles is empty, return
        if uncles_count == 0 {
            return Ok(());
        }

        // if block is genesis, which is expected with zero uncles, return error
        if block.is_genesis() {
            return Err(Error::Uncles(UnclesError::OverCount {
                max: 0,
                actual: uncles_count,
            }));
        }

        // verify uncles length =< max_uncles_num
        let max_uncles_num = self.provider.consensus().max_uncles_num() as u32;
        if uncles_count > max_uncles_num {
            return Err(Error::Uncles(UnclesError::OverCount {
                max: max_uncles_num,
                actual: uncles_count,
            }));
        }

        // verify uncles age
        let max_uncles_age = self.provider.consensus().max_uncles_age() as u64;
        for uncle in block.uncles() {
            let depth = block.header().number().saturating_sub(uncle.number());

            if depth > max_uncles_age || depth < 1 {
                return Err(Error::Uncles(UnclesError::InvalidDepth {
                    min: block.header().number().saturating_sub(max_uncles_age),
                    max: block.header().number().saturating_sub(1),
                    actual: uncle.number(),
                }));
            }
        }

        // cB
        // cB.p^0       1 depth, valid uncle
        // cB.p^1   ---/  2
        // cB.p^2   -----/  3
        // cB.p^3   -------/  4
        // cB.p^4   ---------/  5
        // cB.p^5   -----------/  6
        // cB.p^6   -------------/
        // cB.p^7
        // verify uncles is not included in main chain
        // TODO: cache context
        let mut excluded = FnvHashSet::default();
        let mut included = FnvHashSet::default();
        excluded.insert(block.header().hash().to_owned());
        let mut block_hash = block.header().parent_hash().to_owned();
        excluded.insert(block_hash.clone());
        for _ in 0..max_uncles_age {
            if let Some(header) = self.provider.block_header(&block_hash) {
                let parent_hash = header.parent_hash().to_owned();
                excluded.insert(parent_hash.clone());
                if let Some(uncles) = self.provider.uncles(&block_hash) {
                    uncles.iter().for_each(|uncle| {
                        excluded.insert(uncle.header.hash().to_owned());
                    });
                };
                block_hash = parent_hash;
            } else {
                break;
            }
        }

        for uncle in block.uncles() {
            if uncle.header().difficulty() != self.epoch.difficulty() {
                return Err(Error::Uncles(UnclesError::InvalidDifficulty));
            }

            if self.epoch.number() != uncle.header().epoch() {
                return Err(Error::Uncles(UnclesError::InvalidDifficultyEpoch));
            }

            let uncle_header = uncle.header.clone();

            let uncle_hash = uncle_header.hash().to_owned();
            if included.contains(&uncle_hash) {
                return Err(Error::Uncles(UnclesError::Duplicate(uncle_hash.clone())));
            }

            if excluded.contains(&uncle_hash) {
                return Err(Error::Uncles(UnclesError::InvalidInclude(
                    uncle_hash.clone(),
                )));
            }

            if uncle_header.proposals_hash() != &uncle.cal_proposals_hash() {
                return Err(Error::Uncles(UnclesError::ProposalsHash));
            }

            let mut seen = HashSet::with_capacity(uncle.proposals().len());
            if !uncle.proposals().iter().all(|id| seen.insert(id)) {
                return Err(Error::Uncles(UnclesError::ProposalDuplicate));
            }

            if !self
                .provider
                .consensus()
                .pow_engine()
                .verify_header(&uncle_header)
            {
                return Err(Error::Uncles(UnclesError::InvalidProof));
            }

            included.insert(uncle_hash);
        }

        Ok(())
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
            Err(Error::ExceededMaximumProposalsLimit)
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
        let block_bytes = block.serialized_size(self.proof_size) as u64;
        if block_bytes <= self.block_bytes_limit {
            Ok(())
        } else {
            Err(Error::ExceededMaximumBlockBytes)
        }
    }
}
