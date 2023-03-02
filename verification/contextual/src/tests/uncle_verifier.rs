#![allow(clippy::field_reassign_with_default)]

use crate::contextual_block_verifier::{UncleVerifierContext, VerifyContext};
use crate::uncles_verifier::UnclesVerifier;
use ckb_chain::chain::{ChainController, ChainService};
use ckb_chain_spec::consensus::Consensus;
use ckb_error::assert_error_eq;
use ckb_launcher::SharedBuilder;
use ckb_shared::Shared;
use ckb_store::{ChainDB, ChainStore};
use ckb_types::{
    core::{
        BlockBuilder, BlockNumber, BlockView, EpochExt, HeaderView, TransactionBuilder,
        TransactionView, UncleBlockView,
    },
    packed::{CellInput, ProposalShortId, Script},
    prelude::*,
};
use ckb_verification::UnclesError;
use ckb_verification_traits::Switch;
use rand::random;
use std::sync::Arc;

fn gen_block(parent_header: &HeaderView, nonce: u128, epoch: &EpochExt) -> BlockView {
    let now = parent_header.timestamp() + 1;
    let number = parent_header.number() + 1;
    let cellbase = create_cellbase(number);
    BlockBuilder::default()
        .transaction(cellbase)
        .proposal(ProposalShortId::from_slice(&[1; 10]).unwrap())
        .parent_hash(parent_header.hash())
        .timestamp(now.pack())
        .epoch(epoch.number_with_fraction(number).pack())
        .number(number.pack())
        .compact_target(epoch.compact_target().pack())
        .nonce(nonce.pack())
        .build()
}

fn start_chain(consensus: Option<Consensus>) -> (ChainController, Shared) {
    let mut builder = SharedBuilder::with_temp_db();
    if let Some(consensus) = consensus {
        builder = builder.consensus(consensus);
    }
    let (shared, mut pack) = builder.build().unwrap();

    let chain_service = ChainService::new(shared.clone(), pack.take_proposal_table());
    let chain_controller = chain_service.start::<&str>(None);
    (chain_controller, shared)
}

fn create_cellbase(number: BlockNumber) -> TransactionView {
    TransactionBuilder::default()
        .witness(Script::default().into_witness())
        .input(CellInput::new_cellbase_input(number))
        .output(Default::default())
        .output_data(Default::default())
        .build()
}

fn prepare() -> (Shared, Vec<BlockView>, Vec<BlockView>) {
    let _faketime_guard = ckb_systemtime::faketime();
    _faketime_guard.set_faketime(0);

    let mut consensus = Consensus::default();
    consensus.max_block_proposals_limit = 3;
    consensus.genesis_epoch_ext.set_length(10);

    let (chain_controller, shared) = start_chain(Some(consensus));

    let number = 20;
    let mut chain1: Vec<BlockView> = Vec::new();
    let mut chain2: Vec<BlockView> = Vec::new();

    _faketime_guard.set_faketime(10);

    let genesis = shared
        .store()
        .get_block_header(&shared.store().get_block_hash(0).unwrap())
        .unwrap();

    let mut parent = genesis.clone();
    for _ in 1..number {
        let snapshot = shared.snapshot();
        let epoch = shared
            .consensus()
            .next_epoch_ext(&parent, &snapshot.borrow_as_data_loader())
            .unwrap()
            .epoch();
        let new_block = gen_block(&parent, random(), &epoch);
        chain_controller
            .internal_process_block(Arc::new(new_block.clone()), Switch::DISABLE_ALL)
            .expect("process block ok");
        chain1.push(new_block.clone());
        parent = new_block.header();
    }

    parent = genesis;

    // if block_number < 11 { chain1 == chain2 } else { chain1 != chain2 }
    for i in 1..number {
        let snapshot = shared.snapshot();
        let epoch = shared
            .consensus()
            .next_epoch_ext(&parent, &snapshot.borrow_as_data_loader())
            .unwrap()
            .epoch();
        let new_block = if i > 10 {
            gen_block(&parent, random(), &epoch)
        } else {
            chain1[(i - 1) as usize].clone()
        };
        chain_controller
            .internal_process_block(Arc::new(new_block.clone()), Switch::DISABLE_ALL)
            .expect("process block ok");
        chain2.push(new_block.clone());
        parent = new_block.header();
    }

    // According to the first-received policy, chain1 is the main chain
    (shared, chain1, chain2)
}

fn dummy_context(shared: &Shared) -> VerifyContext<ChainDB> {
    VerifyContext::new(Arc::new(shared.store().clone()), shared.cloned_consensus())
}

fn epoch(shared: &Shared, chain: &[BlockView], index: usize) -> EpochExt {
    let snapshot = shared.snapshot();
    shared
        .consensus()
        .next_epoch_ext(&chain[index].header(), &snapshot.borrow_as_data_loader())
        .unwrap()
        .epoch()
}

// Uncle is ancestor block
#[test]
fn test_double_inclusion() {
    let (shared, chain1, _) = prepare();
    let dummy_context = dummy_context(&shared);

    let block_number = 7;
    let uncle_number = 2;

    let block = chain1[block_number]
        .to_owned()
        .as_advanced_builder()
        .uncle(chain1[uncle_number].to_owned().as_uncle())
        .build();

    let epoch = epoch(&shared, &chain1, block_number - 1);
    let uncle_verifier_context = UncleVerifierContext::new(&dummy_context, &epoch);
    let verifier = UnclesVerifier::new(uncle_verifier_context, &block);

    assert_error_eq!(
        verifier.verify().unwrap_err(),
        UnclesError::DoubleInclusion(block.uncles().get(0).unwrap().header().hash()),
    );
}

// Uncle.compact_target != block.compact_target
#[test]
fn test_invalid_target() {
    let (shared, chain1, chain2) = prepare();
    let dummy_context = dummy_context(&shared);
    let epoch = epoch(&shared, &chain1, 17);
    let invalid_target = epoch.compact_target() + 1;

    let uncle = chain2[16]
        .clone()
        .as_advanced_builder()
        .compact_target(invalid_target.pack())
        .build()
        .as_uncle();
    let block = chain2[18]
        .clone()
        .as_advanced_builder()
        .uncle(uncle)
        .build();

    let uncle_verifier_context = UncleVerifierContext::new(&dummy_context, &epoch);
    let verifier = UnclesVerifier::new(uncle_verifier_context, &block);
    assert_error_eq!(verifier.verify().unwrap_err(), UnclesError::InvalidTarget);
}
// Uncle.epoch != block.epoch
#[test]
fn test_invalid_epoch() {
    let (shared, chain1, chain2) = prepare();
    let dummy_context = dummy_context(&shared);

    let block_number = shared.consensus().genesis_epoch_ext().length() as usize + 2; // epoch = 1
    let uncle_number = shared.consensus().genesis_epoch_ext().length() as usize - 2; // epoch = 0

    let uncle = chain2[uncle_number]
        .clone()
        .as_advanced_builder()
        .compact_target(chain1[block_number].compact_target().pack())
        .build()
        .as_uncle();
    let block = chain1[block_number]
        .clone()
        .as_advanced_builder()
        .uncle(uncle)
        .build();

    let epoch = epoch(&shared, &chain1, block_number - 1);
    let uncle_verifier_context = UncleVerifierContext::new(&dummy_context, &epoch);
    let verifier = UnclesVerifier::new(uncle_verifier_context, &block);

    assert_error_eq!(
        verifier.verify().unwrap_err(),
        UnclesError::InvalidDifficultyEpoch,
    );
}

// Uncle.number >= block.number
#[test]
fn test_invalid_number() {
    let (shared, chain1, chain2) = prepare();
    let dummy_context = dummy_context(&shared);

    let uncle = chain2[18].as_uncle();

    let block = chain1[17]
        .clone()
        .as_advanced_builder()
        .uncle(uncle)
        .header(chain1[17].header())
        .build();

    let epoch = epoch(&shared, &chain1, 16);
    let uncle_verifier_context = UncleVerifierContext::new(&dummy_context, &epoch);
    let verifier = UnclesVerifier::new(uncle_verifier_context, &block);
    assert_error_eq!(verifier.verify().unwrap_err(), UnclesError::InvalidNumber);
}

// Uncle proposals_hash is invalid
#[test]
fn test_uncle_proposals_hash() {
    let (shared, chain1, chain2) = prepare();
    let dummy_context = dummy_context(&shared);
    let block_number = 17;

    let uncle = chain2[16]
        .to_owned()
        .as_advanced_builder()
        .parent_hash(chain1[15].hash())
        .proposal([1; 10].pack())
        .build_unchecked()
        .as_uncle();
    let block = chain1[18]
        .to_owned()
        .as_advanced_builder()
        .uncle(uncle)
        .build();

    let epoch = epoch(&shared, &chain1, block_number);
    let uncle_verifier_context = UncleVerifierContext::new(&dummy_context, &epoch);
    let verifier = UnclesVerifier::new(uncle_verifier_context, &block);
    assert_error_eq!(verifier.verify().unwrap_err(), UnclesError::ProposalsHash);
}

// Uncle contains duplicated proposals
#[test]
fn test_uncle_duplicated_proposals() {
    let (shared, chain1, chain2) = prepare();
    let dummy_context = dummy_context(&shared);

    // All the blocks in chain2 had a proposal before: ProposalShortId::from_slice(&[1; 10]
    let uncle = chain2[6]
        .to_owned()
        .as_advanced_builder()
        .proposal([1; 10].pack())
        .build()
        .as_uncle();
    let block = chain1[8]
        .to_owned()
        .as_advanced_builder()
        .uncle(uncle)
        .build();

    let epoch = epoch(&shared, &chain2, 7);
    let uncle_verifier_context = UncleVerifierContext::new(&dummy_context, &epoch);
    let verifier = UnclesVerifier::new(uncle_verifier_context, &block);
    assert_error_eq!(
        verifier.verify().unwrap_err(),
        UnclesError::ProposalDuplicate,
    );
}

// Duplicated uncles
#[test]
fn test_duplicated_uncles() {
    let (shared, chain1, chain2) = prepare();
    let dummy_context = dummy_context(&shared);

    let uncle = chain2[10].as_uncle();
    let duplicated_uncles = vec![uncle.clone(), uncle];

    let block = chain1[12]
        .to_owned()
        .as_advanced_builder()
        .uncles(duplicated_uncles)
        .build();

    let epoch = epoch(&shared, &chain1, 11);
    let uncle_verifier_context = UncleVerifierContext::new(&dummy_context, &epoch);
    let verifier = UnclesVerifier::new(uncle_verifier_context, &block);
    assert_error_eq!(
        verifier.verify().unwrap_err(),
        UnclesError::Duplicate(block.uncles().get(1).unwrap().header().hash()),
    );
}

// Uncles count exceeds limit
#[test]
fn test_uncle_over_count() {
    let (shared, chain1, chain2) = prepare();
    let dummy_context = dummy_context(&shared);

    let max_uncles_num = shared.consensus().max_uncles_num();
    let mut uncles: Vec<UncleBlockView> = Vec::new();
    for _ in 0..=max_uncles_num {
        let uncle = chain2[10].clone().as_uncle();
        uncles.push(uncle);
    }
    let block = chain1[12]
        .clone()
        .as_advanced_builder()
        .uncles(uncles)
        .build();
    // uncle overcount

    let epoch = epoch(&shared, &chain1, 11);
    let uncle_verifier_context = UncleVerifierContext::new(&dummy_context, &epoch);
    let verifier = UnclesVerifier::new(uncle_verifier_context, &block);
    assert_error_eq!(
        verifier.verify().unwrap_err(),
        UnclesError::OverCount {
            max: max_uncles_num as u32,
            actual: max_uncles_num as u32 + 1,
        },
    );
}

#[test]
fn test_exceeded_maximum_proposals_limit() {
    let (shared, chain1, chain2) = prepare();
    let dummy_context = dummy_context(&shared);

    let uncle = chain2[6]
        .clone()
        .as_advanced_builder()
        .proposals(vec![
            ProposalShortId::from_slice(&[1; 10]).unwrap(),
            ProposalShortId::from_slice(&[2; 10]).unwrap(),
            ProposalShortId::from_slice(&[3; 10]).unwrap(),
            ProposalShortId::from_slice(&[4; 10]).unwrap(),
        ])
        .build()
        .as_uncle();

    let block = chain1[8].clone().as_advanced_builder().uncle(uncle).build();

    let epoch = epoch(&shared, &chain1, 7);
    let uncle_verifier_context = UncleVerifierContext::new(&dummy_context, &epoch);
    let verifier = UnclesVerifier::new(uncle_verifier_context, &block);
    assert_error_eq!(
        verifier.verify().unwrap_err(),
        UnclesError::ExceededMaximumProposalsLimit,
    );
}

#[test]
fn test_descendant_limit() {
    let (shared, chain1, chain2) = prepare();
    let dummy_context = dummy_context(&shared);

    {
        let uncle = chain2[16].clone().as_uncle();
        let block = chain1[18]
            .clone()
            .as_advanced_builder()
            .uncle(uncle)
            .build();

        let epoch = epoch(&shared, &chain1, 17);
        let uncle_verifier_context = UncleVerifierContext::new(&dummy_context, &epoch);
        let verifier = UnclesVerifier::new(uncle_verifier_context, &block);
        assert_error_eq!(verifier.verify().unwrap_err(), UnclesError::DescendantLimit);
    }

    // embedded should be ok
    {
        let uncle1 = chain2[15]
            .clone()
            .as_advanced_builder()
            .parent_hash(chain1[14].hash())
            .build()
            .as_uncle();
        let uncle2 = chain2[16]
            .clone()
            .as_advanced_builder()
            .parent_hash(uncle1.hash())
            .build()
            .as_uncle();
        let block = chain1[18]
            .clone()
            .as_advanced_builder()
            .uncle(uncle1)
            .uncle(uncle2)
            .build();

        let epoch = epoch(&shared, &chain1, 17);
        let uncle_verifier_context = UncleVerifierContext::new(&dummy_context, &epoch);
        let verifier = UnclesVerifier::new(uncle_verifier_context, &block);
        assert!(verifier.verify().is_ok());
    }
}

#[test]
fn test_descendant_continuity() {
    let (shared, chain1, chain2) = prepare();
    let dummy_context = dummy_context(&shared);

    {
        let uncle = chain2[16]
            .clone()
            .as_advanced_builder()
            .parent_hash(chain1[14].hash())
            .build()
            .as_uncle();
        let block = chain1[18]
            .clone()
            .as_advanced_builder()
            .uncle(uncle)
            .build();

        let epoch = epoch(&shared, &chain1, 17);
        let uncle_verifier_context = UncleVerifierContext::new(&dummy_context, &epoch);
        let verifier = UnclesVerifier::new(uncle_verifier_context, &block);
        assert_error_eq!(verifier.verify().unwrap_err(), UnclesError::DescendantLimit);
    }
}

#[test]
fn test_ok() {
    let (shared, chain1, chain2) = prepare();
    let dummy_context = dummy_context(&shared);

    {
        let uncle = chain2[16]
            .clone()
            .as_advanced_builder()
            .parent_hash(chain1[15].hash())
            .build()
            .as_uncle();
        let block = chain1[18]
            .clone()
            .as_advanced_builder()
            .uncle(uncle)
            .build();

        let epoch = epoch(&shared, &chain1, 17);
        let uncle_verifier_context = UncleVerifierContext::new(&dummy_context, &epoch);
        let verifier = UnclesVerifier::new(uncle_verifier_context, &block);
        assert!(verifier.verify().is_ok());
    }

    {
        let uncle = chain2[10].clone().as_uncle();
        let block = chain1[12]
            .clone()
            .as_advanced_builder()
            .uncle(uncle)
            .build();

        let epoch = epoch(&shared, &chain1, 11);
        let uncle_verifier_context = UncleVerifierContext::new(&dummy_context, &epoch);
        let verifier = UnclesVerifier::new(uncle_verifier_context, &block);
        assert!(verifier.verify().is_ok());
    }
}

#[test]
fn test_uncle_with_uncle_descendant() {
    let (_, chain1, chain2) = prepare();

    let mut consensus = Consensus::default();
    consensus.max_block_proposals_limit = 3;
    consensus.genesis_epoch_ext.set_length(10);
    let (controller, shared) = start_chain(Some(consensus));

    for block in &chain2 {
        controller
            .internal_process_block(Arc::new(block.clone()), Switch::DISABLE_ALL)
            .expect("process block ok");
    }

    let uncle10 = chain1[10].clone().as_uncle();
    let parent = chain2.last().cloned().unwrap().header();
    let epoch18 = epoch(&shared, &chain2, 18); // last index 18
    let block = gen_block(&parent, random(), &epoch18)
        .as_advanced_builder()
        .uncle(uncle10)
        .build();

    controller
        .internal_process_block(Arc::new(block.clone()), Switch::DISABLE_ALL)
        .expect("process block ok");

    {
        let parent = block.header();
        let uncle11 = chain1[11].clone().as_uncle();
        let epoch18 = epoch(&shared, &chain2, 18);
        let new_block = gen_block(&parent, random(), &epoch18)
            .as_advanced_builder()
            .uncle(uncle11)
            .build();

        let dummy_context = dummy_context(&shared);
        let uncle_verifier_context = UncleVerifierContext::new(&dummy_context, &epoch18);
        let verifier = UnclesVerifier::new(uncle_verifier_context, &new_block);
        assert!(verifier.verify().is_ok());
    }
}
