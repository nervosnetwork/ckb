use crate::contextual_block_verifier::{ForkContext, UncleVerifierContext};
use crate::error::{Error, UnclesError};
use crate::uncles_verifier::UnclesVerifier;
use ckb_chain::chain::{ChainController, ChainService};
use ckb_chain_spec::consensus::Consensus;
use ckb_core::block::{Block, BlockBuilder};
use ckb_core::extras::EpochExt;
use ckb_core::header::{Header, HeaderBuilder};
use ckb_core::script::Script;
use ckb_core::transaction::{
    CellInput, CellOutput, ProposalShortId, Transaction, TransactionBuilder,
};
use ckb_core::uncle::uncles_hash;
use ckb_core::{BlockNumber, Bytes, Capacity};
use ckb_db::memorydb::MemoryKeyValueDB;
use ckb_notify::NotifyService;
use ckb_shared::shared::{Shared, SharedBuilder};
use ckb_store::ChainKVStore;
use ckb_traits::ChainProvider;
#[cfg(not(disable_faketime))]
use faketime;
use numext_fixed_hash::H256;
use std::sync::Arc;

fn gen_block(parent_header: &Header, nonce: u64, epoch: &EpochExt) -> Block {
    let now = 1 + parent_header.timestamp();
    let number = parent_header.number() + 1;
    let cellbase = create_cellbase(number);
    let header_builder = HeaderBuilder::default()
        .parent_hash(parent_header.hash().to_owned())
        .timestamp(now)
        .epoch(epoch.number())
        .number(number)
        .difficulty(epoch.difficulty().clone())
        .nonce(nonce);

    BlockBuilder::default()
        .transaction(cellbase)
        .proposal(ProposalShortId::from_slice(&[1; 10]).unwrap())
        .header_builder(header_builder)
        .build()
}

fn start_chain(
    consensus: Option<Consensus>,
) -> (ChainController, Shared<ChainKVStore<MemoryKeyValueDB>>) {
    let mut builder = SharedBuilder::<MemoryKeyValueDB>::new();
    if let Some(consensus) = consensus {
        builder = builder.consensus(consensus);
    }
    let shared = builder.build().unwrap();

    let notify = NotifyService::default().start::<&str>(None);
    let chain_service = ChainService::new(shared.clone(), notify);
    let chain_controller = chain_service.start::<&str>(None);
    (chain_controller, shared)
}

fn create_cellbase(number: BlockNumber) -> Transaction {
    TransactionBuilder::default()
        .input(CellInput::new_cellbase_input(number))
        .output(CellOutput::new(
            Capacity::zero(),
            Bytes::default(),
            Script::default(),
            None,
        ))
        .build()
}

#[cfg(not(disable_faketime))]
#[test]
fn test_uncle_verifier() {
    let faketime_file = faketime::millis_tempfile(0).expect("create faketime file");
    faketime::enable(&faketime_file);

    let mut consensus = Consensus::default();
    consensus.max_block_proposals_limit = 3;
    consensus.genesis_epoch_ext.set_length(10);

    let (chain_controller, shared) = start_chain(Some(consensus));

    let dummy_context = ForkContext {
        attached_blocks: vec![],
        detached_blocks: vec![],
        provider: shared.clone(),
    };

    let number = 20;
    let mut chain1: Vec<Block> = Vec::new();
    let mut chain2: Vec<Block> = Vec::new();

    faketime::write_millis(&faketime_file, 10).expect("write millis");

    let mut parent = shared.block_header(&shared.block_hash(0).unwrap()).unwrap();
    for i in 1..number {
        let parent_epoch = shared.get_block_epoch(&parent.hash()).unwrap();
        let epoch = shared
            .next_epoch_ext(&parent_epoch, &parent)
            .unwrap_or(parent_epoch);
        let new_block = gen_block(&parent, i, &epoch);
        chain_controller
            .process_block(Arc::new(new_block.clone()), false)
            .expect("process block ok");
        chain1.push(new_block.clone());
        parent = new_block.header().to_owned();
    }

    parent = shared.block_header(&shared.block_hash(0).unwrap()).unwrap();

    // if block_number < 11 { chain1 == chain2 } else { chain1 != chain2 }
    for i in 1..number {
        let parent_epoch = shared.get_block_epoch(&parent.hash()).unwrap();
        let epoch = shared
            .next_epoch_ext(&parent_epoch, &parent)
            .unwrap_or(parent_epoch);
        let new_block = if i > 10 {
            gen_block(&parent, i + 1000, &epoch)
        } else {
            chain1[(i - 1) as usize].clone()
        };
        chain_controller
            .process_block(Arc::new(new_block.clone()), false)
            .expect("process block ok");
        chain2.push(new_block.clone());
        parent = new_block.header().to_owned();
    }

    // Uncles not match uncles_count
    {
        let parent_epoch = shared
            .get_block_epoch(&chain1[chain1.len() - 2].header().hash())
            .unwrap();
        let epoch = shared
            .next_epoch_ext(&parent_epoch, chain1[chain1.len() - 2].header())
            .unwrap_or(parent_epoch);
        let block = unsafe {
            BlockBuilder::from_block(chain1.last().cloned().unwrap())
                .uncle(chain2.last().cloned().unwrap().into())
                .build_unchecked()
        };

        let uncle_verifier_context = UncleVerifierContext::new(&dummy_context, &epoch, &block);
        let verifier = UnclesVerifier::new(uncle_verifier_context, &block);
        assert_eq!(
            verifier.verify(),
            Err(Error::Uncles(UnclesError::MissMatchCount {
                expected: 0,
                actual: 1
            }))
        );
    }

    // Uncles not match uncles_hash
    {
        let parent_epoch = shared
            .get_block_epoch(&chain1[chain1.len() - 2].header().hash())
            .unwrap();
        let epoch = shared
            .next_epoch_ext(&parent_epoch, chain1[chain1.len() - 2].header())
            .unwrap_or(parent_epoch);
        // header has 1 uncle, but body has empty uncles
        let block = unsafe {
            BlockBuilder::from_block(chain1.last().cloned().unwrap())
                .header(HeaderBuilder::default().uncles_count(1).build())
                .uncle(chain2.last().cloned().unwrap().into())
                .build_unchecked()
        };
        let uncle_verifier_context = UncleVerifierContext::new(&dummy_context, &epoch, &block);
        let verifier = UnclesVerifier::new(uncle_verifier_context, &block);
        assert_eq!(
            verifier.verify(),
            Err(Error::Uncles(UnclesError::InvalidHash {
                expected: H256::zero(),
                actual: block.cal_uncles_hash()
            }))
        );

        // header has empty uncles, but body has 1 uncle
        let uncles_hash = uncles_hash(&[chain2.last().cloned().unwrap().into()]);
        let block = unsafe {
            BlockBuilder::from_block(chain1.last().cloned().unwrap())
                .header(
                    HeaderBuilder::from_header(chain1.last().cloned().unwrap().header().to_owned())
                        .uncles_count(0)
                        .uncles_hash(uncles_hash.clone())
                        .build(),
                )
                .build_unchecked()
        };
        let uncle_verifier_context = UncleVerifierContext::new(&dummy_context, &epoch, &block);
        let verifier = UnclesVerifier::new(uncle_verifier_context, &block);
        assert_eq!(
            verifier.verify(),
            Err(Error::Uncles(UnclesError::InvalidHash {
                expected: uncles_hash,
                actual: H256::zero(),
            }))
        );
    }

    // Uncle is ancestor block
    {
        let block_number = 7;
        let uncle_number = 2;

        let parent_epoch = shared
            .get_block_epoch(&chain1[block_number - 1].header().hash())
            .unwrap();
        let epoch = shared
            .next_epoch_ext(&parent_epoch, chain1[block_number - 1].header())
            .unwrap_or(parent_epoch);

        let block = BlockBuilder::from_block(chain1[block_number].to_owned())
            .uncle(chain1[uncle_number].to_owned().into())
            .header_builder(HeaderBuilder::from_header(
                chain1[block_number].header().to_owned(),
            ))
            .build();

        let uncle_verifier_context = UncleVerifierContext::new(&dummy_context, &epoch, &block);
        let verifier = UnclesVerifier::new(uncle_verifier_context, &block);
        assert_eq!(
            verifier.verify(),
            Err(Error::Uncles(UnclesError::DoubleInclusion(
                block.uncles()[0].header().hash().to_owned()
            )))
        );
    }

    // Uncle.epoch != block.epoch
    {
        let block_number = shared.consensus().genesis_epoch_ext().length() as usize + 2; // epoch = 1
        let uncle_number = shared.consensus().genesis_epoch_ext().length() as usize - 2; // epoch = 0

        let parent_epoch = shared
            .get_block_epoch(&chain1[block_number - 1].header().hash())
            .unwrap();
        let epoch = shared
            .next_epoch_ext(&parent_epoch, chain1[block_number - 1].header())
            .unwrap_or(parent_epoch);

        let block = BlockBuilder::from_block(chain1.get(block_number).cloned().unwrap()) // epoch 1
            .uncle(chain2.get(uncle_number).cloned().unwrap().into())                    // epoch 0
            .header_builder(HeaderBuilder::from_header(
                chain1[block_number].header().to_owned()
            )).build();
        let uncle_verifier_context = UncleVerifierContext::new(&dummy_context, &epoch, &block);
        let verifier = UnclesVerifier::new(uncle_verifier_context, &block);
        assert_eq!(
            verifier.verify(),
            Err(Error::Uncles(UnclesError::InvalidDifficultyEpoch))
        );
    }

    // Uncle.number >= block.number
    {
        let parent_epoch = shared.get_block_epoch(&chain2[17].header().hash()).unwrap();
        let epoch = shared
            .next_epoch_ext(&parent_epoch, chain2[17].header())
            .unwrap_or(parent_epoch);

        let uncle = unsafe {
            BlockBuilder::from_block(chain1[16].to_owned())
                .header_builder(
                    HeaderBuilder::from_header(chain1[16].header().to_owned()).number(20),
                )
                .build_unchecked()
        };
        let block = BlockBuilder::from_block(chain2[18].to_owned())
            .uncle(uncle.into())
            .header_builder(HeaderBuilder::from_header(chain2[18].header().to_owned()))
            .build();
        let uncle_verifier_context = UncleVerifierContext::new(&dummy_context, &epoch, &block);
        let verifier = UnclesVerifier::new(uncle_verifier_context, &block);
        assert_eq!(
            verifier.verify(),
            Err(Error::Uncles(UnclesError::InvalidNumber))
        );
    }

    // Uncle proposals_hash is invalid
    {
        let parent_epoch = shared.get_block_epoch(&chain2[17].header().hash()).unwrap();
        let epoch = shared
            .next_epoch_ext(&parent_epoch, chain2[17].header())
            .unwrap_or(parent_epoch);

        let uncle = unsafe {
            BlockBuilder::from_block(chain1[16].to_owned())
                .proposal(ProposalShortId::from_slice(&[1; 10]).unwrap())
                .build_unchecked()
        };
        let block = BlockBuilder::from_block(chain2[18].to_owned())
            .uncle(uncle.into())
            .header_builder(HeaderBuilder::from_header(chain2[18].header().to_owned()))
            .build();
        let uncle_verifier_context = UncleVerifierContext::new(&dummy_context, &epoch, &block);
        let verifier = UnclesVerifier::new(uncle_verifier_context, &block);
        assert_eq!(
            verifier.verify(),
            Err(Error::Uncles(UnclesError::ProposalsHash))
        );
    }

    // Uncle contains duplicated proposals
    {
        let parent_epoch = shared.get_block_epoch(&chain1[7].header().hash()).unwrap();
        let epoch = shared
            .next_epoch_ext(&parent_epoch, chain1[7].header())
            .unwrap_or(parent_epoch);

        let uncle = BlockBuilder::from_block(chain2[6].to_owned())
            .proposal(ProposalShortId::from_slice(&[1; 10]).unwrap())
            .header_builder(HeaderBuilder::from_header(chain2[6].header().to_owned()))
            .build();
        let block = BlockBuilder::from_block(chain1[8].to_owned())
            .uncle(uncle.into())
            .header_builder(HeaderBuilder::from_header(chain1[8].header().to_owned()))
            .build();
        let uncle_verifier_context = UncleVerifierContext::new(&dummy_context, &epoch, &block);
        let verifier = UnclesVerifier::new(uncle_verifier_context, &block);
        assert_eq!(
            verifier.verify(),
            Err(Error::Uncles(UnclesError::ProposalDuplicate))
        );
    }

    // Duplicated uncles
    {
        let parent_epoch = shared.get_block_epoch(&chain1[11].header().hash()).unwrap();
        let epoch = shared
            .next_epoch_ext(&parent_epoch, chain1[11].header())
            .unwrap_or(parent_epoch);

        let uncle1 = BlockBuilder::from_block(chain1[10].to_owned())
            .header_builder(HeaderBuilder::from_header(chain1[10].header().to_owned()))
            .build();
        let uncle2 = BlockBuilder::from_block(chain1[10].to_owned())
            .header_builder(HeaderBuilder::from_header(chain1[10].header().to_owned()))
            .build();
        let block = BlockBuilder::from_block(chain2[12].to_owned())
            .uncle(uncle1.into())
            .uncle(uncle2.into())
            .header_builder(HeaderBuilder::from_header(chain2[12].header().to_owned()))
            .build();
        // uncle duplicate
        let uncle_verifier_context = UncleVerifierContext::new(&dummy_context, &epoch, &block);
        let verifier = UnclesVerifier::new(uncle_verifier_context, &block);
        assert_eq!(
            verifier.verify(),
            Err(Error::Uncles(UnclesError::Duplicate(
                block.uncles()[1].header().hash().to_owned()
            )))
        );
    }

    // Uncles count exceeds limit
    {
        let parent_epoch = shared.get_block_epoch(&chain1[11].header().hash()).unwrap();
        let epoch = shared
            .next_epoch_ext(&parent_epoch, chain1[11].header())
            .unwrap_or(parent_epoch);

        let max_uncles_num = shared.consensus().max_uncles_num();
        let mut uncles = Vec::new();
        for _ in 0..=max_uncles_num {
            let uncle = BlockBuilder::from_block(chain1[10].to_owned())
                .header_builder(HeaderBuilder::from_header(chain1[10].header().to_owned()))
                .build();
            uncles.push(uncle.into());
        }
        let block = BlockBuilder::from_block(chain2[12].to_owned())
            .uncles(uncles)
            .header_builder(HeaderBuilder::from_header(chain2[12].header().to_owned()))
            .build();
        // uncle overcount
        let uncle_verifier_context = UncleVerifierContext::new(&dummy_context, &epoch, &block);
        let verifier = UnclesVerifier::new(uncle_verifier_context, &block);
        assert_eq!(
            verifier.verify(),
            Err(Error::Uncles(UnclesError::OverCount {
                max: max_uncles_num as u32,
                actual: max_uncles_num as u32 + 1
            }))
        );
    }

    {
        let parent_epoch = shared.get_block_epoch(&chain1[7].header().hash()).unwrap();
        let epoch = shared
            .next_epoch_ext(&parent_epoch, chain1[7].header())
            .unwrap_or(parent_epoch);
        let uncle = BlockBuilder::from_block(chain2[6].clone())
            .proposals(vec![
                ProposalShortId::from_slice(&[1; 10]).unwrap(),
                ProposalShortId::from_slice(&[2; 10]).unwrap(),
                ProposalShortId::from_slice(&[3; 10]).unwrap(),
                ProposalShortId::from_slice(&[4; 10]).unwrap(),
            ])
            .header_builder(HeaderBuilder::from_header(chain2[6].header().to_owned()))
            .build();

        let block = BlockBuilder::from_block(chain1[8].clone())
            .uncle(uncle.into())
            .header_builder(HeaderBuilder::from_header(chain1[8].header().to_owned()))
            .build();
        let uncle_verifier_context = UncleVerifierContext::new(&dummy_context, &epoch, &block);
        let verifier = UnclesVerifier::new(uncle_verifier_context, &block);
        assert_eq!(
            verifier.verify(),
            Err(Error::Uncles(UnclesError::ExceededMaximumProposalsLimit))
        );
    }

    let parent_epoch = shared.get_block_epoch(&chain2[17].header().hash()).unwrap();
    let epoch = shared
        .next_epoch_ext(&parent_epoch, chain2[17].header())
        .unwrap_or(parent_epoch);

    let uncle = BlockBuilder::from_block(chain1[16].clone())
        .header_builder(HeaderBuilder::from_header(chain1[16].header().to_owned()))
        .build();
    let block = BlockBuilder::from_block(chain2[18].clone())
        .uncle(uncle.into())
        .header_builder(HeaderBuilder::from_header(chain2[18].header().to_owned()))
        .build();
    let uncle_verifier_context = UncleVerifierContext::new(&dummy_context, &epoch, &block);
    let verifier = UnclesVerifier::new(uncle_verifier_context, &block);
    assert_eq!(verifier.verify(), Ok(()));

    let parent_epoch = shared.get_block_epoch(&chain1[11].header().hash()).unwrap();
    let epoch = shared
        .next_epoch_ext(&parent_epoch, chain1[11].header())
        .unwrap_or(parent_epoch);

    let uncle = BlockBuilder::from_block(chain1[10].clone())
        .header_builder(HeaderBuilder::from_header(chain1[10].header().to_owned()))
        .build();
    let block = BlockBuilder::from_block(chain2[12].clone())
        .uncle(uncle.into())
        .header_builder(HeaderBuilder::from_header(chain2[12].header().to_owned()))
        .build();
    let uncle_verifier_context = UncleVerifierContext::new(&dummy_context, &epoch, &block);
    let verifier = UnclesVerifier::new(uncle_verifier_context, &block);
    assert_eq!(verifier.verify(), Ok(()));
}

#[cfg(not(disable_faketime))]
#[test]
fn test_uncle_verifier_with_fork_context() {
    let faketime_file = faketime::millis_tempfile(0).expect("create faketime file");
    faketime::enable(&faketime_file);

    let mut consensus = Consensus::default();
    consensus.max_block_proposals_limit = 3;
    consensus.genesis_epoch_ext.set_length(10);

    let (chain_controller, shared) = start_chain(Some(consensus));

    let mut chain1: Vec<Block> = Vec::new();
    let mut chain2: Vec<Block> = Vec::new();

    faketime::write_millis(&faketime_file, 10).expect("write millis");

    let mut parent = shared.block_header(&shared.block_hash(0).unwrap()).unwrap();
    for i in 1..20 {
        let parent_epoch = shared.get_block_epoch(&parent.hash()).unwrap();
        let epoch = shared
            .next_epoch_ext(&parent_epoch, &parent)
            .unwrap_or(parent_epoch);
        let new_block = gen_block(&parent, i, &epoch);
        chain_controller
            .process_block(Arc::new(new_block.clone()), false)
            .expect("process block ok");
        chain1.push(new_block.clone());
        parent = new_block.header().to_owned();
    }

    parent = shared.block_header(&shared.block_hash(0).unwrap()).unwrap();

    // if block_number < 11 { chain1 == chain2 } else { chain1 != chain2 }
    for i in 1..19 {
        let parent_epoch = shared.get_block_epoch(&parent.hash()).unwrap();
        let epoch = shared
            .next_epoch_ext(&parent_epoch, &parent)
            .unwrap_or(parent_epoch);
        let new_block = if i > 10 {
            gen_block(&parent, i + 1000, &epoch)
        } else {
            chain1[(i - 1) as usize].clone()
        };
        chain_controller
            .process_block(Arc::new(new_block.clone()), false)
            .expect("process block ok");
        chain2.push(new_block.clone());
        parent = new_block.header().to_owned();
    }

    let parent_epoch = shared.get_block_epoch(&parent.hash()).unwrap();
    let epoch = shared
        .next_epoch_ext(&parent_epoch, &parent)
        .unwrap_or(parent_epoch);
    let new_block = gen_block(&parent, 1019, &epoch);

    let context = ForkContext {
        attached_blocks: chain2[10..18].iter().collect(),
        detached_blocks: chain1[10..19].iter().collect(),
        provider: shared.clone(),
    };

    let uncle = BlockBuilder::from_block(chain2[17].clone())
        .header_builder(HeaderBuilder::from_header(chain2[17].header().to_owned()))
        .build();
    let block = BlockBuilder::from_block(new_block.clone())
        .uncle(uncle.into())
        .header_builder(HeaderBuilder::from_header(new_block.header().to_owned()))
        .build();
    let uncle_verifier_context = UncleVerifierContext::new(&context, &epoch, &block);
    let verifier = UnclesVerifier::new(uncle_verifier_context, &block);
    assert_eq!(
        verifier.verify(),
        Err(Error::Uncles(UnclesError::DoubleInclusion(
            block.uncles()[0].header().hash().to_owned()
        )))
    );

    let uncle = BlockBuilder::from_block(chain1[17].clone())
        .header_builder(HeaderBuilder::from_header(chain1[17].header().to_owned()))
        .build();
    let block = BlockBuilder::from_block(new_block.clone())
        .uncle(uncle.into())
        .header_builder(HeaderBuilder::from_header(new_block.header().to_owned()))
        .build();
    let uncle_verifier_context = UncleVerifierContext::new(&context, &epoch, &block);
    let verifier = UnclesVerifier::new(uncle_verifier_context, &block);
    assert_eq!(verifier.verify(), Ok(()));
}
