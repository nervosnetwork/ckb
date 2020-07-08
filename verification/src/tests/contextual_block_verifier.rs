use crate::contextual_block_verifier::{RewardVerifier, VerifyContext};
use ckb_chain::chain::{ChainController, ChainService};
use ckb_chain_spec::consensus::Consensus;
use ckb_error::assert_error_eq;
use ckb_shared::shared::{Shared, SharedBuilder};
use ckb_store::ChainDB;
use ckb_types::{
    core::{cell::ResolvedTransaction, TransactionBuilder},
    packed::CellInput,
};

use crate::CellbaseError;

fn start_chain(consensus: Option<Consensus>) -> (ChainController, Shared) {
    let mut builder = SharedBuilder::with_temp_db();
    if let Some(consensus) = consensus {
        builder = builder.consensus(consensus);
    }
    let (shared, table) = builder.build().unwrap();

    let chain_service = ChainService::new(shared.clone(), table);
    let chain_controller = chain_service.start::<&str>(None);
    (chain_controller, shared)
}

fn dummy_context(shared: &Shared) -> VerifyContext<'_, ChainDB> {
    VerifyContext::new(shared.store(), shared.consensus())
}

#[test]
pub fn test_should_have_no_output_in_cellbase_no_finalization_target() {
    let (_chain, shared) = start_chain(None);
    let context = dummy_context(&shared);

    let parent = shared.consensus().genesis_block().header();
    let number = parent.number() + 1;
    let cellbase = TransactionBuilder::default()
        .input(CellInput::new_cellbase_input(number))
        .output(Default::default())
        .output_data(Default::default())
        .build();

    let cellbase = ResolvedTransaction {
        transaction: cellbase,
        resolved_cell_deps: vec![],
        resolved_inputs: vec![],
        resolved_dep_groups: vec![],
    };

    let ret = RewardVerifier::new(&context, &[cellbase], &parent).verify();

    assert_error_eq!(ret.unwrap_err(), CellbaseError::InvalidRewardTarget,);
}
