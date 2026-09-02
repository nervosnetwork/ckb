//! Production refinement for the projected-final-owner input relation.

use super::claim_relations::{
    ClaimPoolParent, ClaimRawTransaction, ClaimReleasedInputContext, ClaimReleasedInputCut,
    ClaimReleasedInputDisposition, released_input_disposition,
};
use super::foundation::{
    accept_remote_transaction_with_payload, limits, resolved_payload_with_facts,
};
use crate::authority::{
    plan::{TxPoolAuthority, test_support::ReleasedInputContextForFoundation},
    state::{AcceptedStatus, RawTxHash},
};
use ckb_types::{
    bytes::Bytes,
    core::{Capacity, TransactionBuilder},
    packed::{Byte32, CellInput, CellOutput, OutPoint},
    prelude::Pack,
};
use std::collections::BTreeSet;

fn accepted_parent_and_spender(
    parent_outputs: usize,
) -> (TxPoolAuthority, RawTxHash, RawTxHash, OutPoint) {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let mut parent = TransactionBuilder::default().version(43_001u32);
    for _ in 0..parent_outputs {
        parent = parent
            .output(CellOutput::default())
            .output_data(Bytes::new().pack());
    }
    let parent_tx = parent.build();
    let parent_hash = accept_remote_transaction_with_payload(
        &mut authority,
        parent_tx.clone(),
        43_001,
        AcceptedStatus::Pending,
        resolved_payload_with_facts(&parent_tx, Vec::new(), Vec::new(), Capacity::shannons(1)),
    );
    let input = OutPoint::new(parent_tx.hash(), 0);
    let spender_tx = TransactionBuilder::default()
        .version(43_002u32)
        .input(CellInput::new(input.clone(), 0))
        .build();
    let spender_hash = accept_remote_transaction_with_payload(
        &mut authority,
        spender_tx.clone(),
        43_002,
        AcceptedStatus::Pending,
        resolved_payload_with_facts(&spender_tx, Vec::new(), Vec::new(), Capacity::shannons(1)),
    );
    (authority, parent_hash, spender_hash, input)
}

fn expected(
    context: ClaimReleasedInputContext,
    current_spender: Option<ClaimRawTransaction>,
    removed: BTreeSet<ClaimRawTransaction>,
    chain_backed: bool,
    parent: ClaimPoolParent,
) -> ClaimReleasedInputDisposition {
    released_input_disposition(&ClaimReleasedInputCut {
        context,
        current_spender,
        removed,
        chain_backed,
        parent,
        output_index: 0,
    })
}

#[test]
fn uak_replacement_and_administration_share_the_final_owner_projection() {
    let (authority, parent, spender, input) = accepted_parent_and_spender(1);
    let claim_spender = ClaimRawTransaction(2);

    for candidate_uses_input in [false, true] {
        let production = authority
            .released_input_for_foundation(
                &spender,
                &input,
                std::slice::from_ref(&spender),
                ReleasedInputContextForFoundation::Replacement {
                    candidate_uses_input,
                },
            )
            .expect("the real replacement cohort is structurally valid");
        let claim = expected(
            ClaimReleasedInputContext::Replacement {
                candidate_uses_input,
            },
            Some(claim_spender),
            BTreeSet::from([claim_spender]),
            false,
            ClaimPoolParent::SurvivingAccepted { output_count: 1 },
        );
        assert_eq!(production, claim == ClaimReleasedInputDisposition::Released);
    }

    let administrative = authority
        .released_input_for_foundation(
            &spender,
            &input,
            std::slice::from_ref(&spender),
            ReleasedInputContextForFoundation::Administrative,
        )
        .expect("the real administrative cohort is structurally valid");
    assert_eq!(
        administrative,
        expected(
            ClaimReleasedInputContext::Administrative {
                victim: claim_spender,
            },
            Some(claim_spender),
            BTreeSet::from([claim_spender]),
            false,
            ClaimPoolParent::SurvivingAccepted { output_count: 1 },
        ) == ClaimReleasedInputDisposition::Released
    );

    for context in [
        ReleasedInputContextForFoundation::Replacement {
            candidate_uses_input: false,
        },
        ReleasedInputContextForFoundation::Administrative,
    ] {
        let production = authority
            .released_input_for_foundation(
                &spender,
                &input,
                &[parent.clone(), spender.clone()],
                context,
            )
            .expect("the complete removal cohort is structurally valid");
        let claim_context = match context {
            ReleasedInputContextForFoundation::Replacement {
                candidate_uses_input,
            } => ClaimReleasedInputContext::Replacement {
                candidate_uses_input,
            },
            ReleasedInputContextForFoundation::Administrative => {
                ClaimReleasedInputContext::Administrative {
                    victim: claim_spender,
                }
            }
        };
        assert_eq!(
            production,
            expected(
                claim_context,
                Some(claim_spender),
                BTreeSet::from([claim_spender]),
                false,
                ClaimPoolParent::Removed,
            ) == ClaimReleasedInputDisposition::Released,
            "a removed pool parent cannot back a released input"
        );
    }

    assert!(
        !authority
            .released_input_for_foundation(
                &spender,
                &input,
                std::slice::from_ref(&parent),
                ReleasedInputContextForFoundation::Replacement {
                    candidate_uses_input: false,
                },
            )
            .expect("a retained current spender is an ordinary replacement outcome")
    );
}

#[test]
fn uak_chain_backed_input_survives_without_a_pool_parent_in_both_removal_modes() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let input = OutPoint::new(Byte32::new([43; 32]), 0);
    let spender_tx = TransactionBuilder::default()
        .version(43_003u32)
        .input(CellInput::new(input.clone(), 0))
        .build();
    let spender = accept_remote_transaction_with_payload(
        &mut authority,
        spender_tx.clone(),
        43_003,
        AcceptedStatus::Pending,
        resolved_payload_with_facts(
            &spender_tx,
            Vec::new(),
            vec![input.clone()],
            Capacity::shannons(1),
        ),
    );

    for context in [
        ReleasedInputContextForFoundation::Replacement {
            candidate_uses_input: false,
        },
        ReleasedInputContextForFoundation::Administrative,
    ] {
        let production = authority
            .released_input_for_foundation(
                &spender,
                &input,
                std::slice::from_ref(&spender),
                context,
            )
            .expect("the chain-backed removal cohort is structurally valid");
        let claim_context = match context {
            ReleasedInputContextForFoundation::Replacement {
                candidate_uses_input,
            } => ClaimReleasedInputContext::Replacement {
                candidate_uses_input,
            },
            ReleasedInputContextForFoundation::Administrative => {
                ClaimReleasedInputContext::Administrative {
                    victim: ClaimRawTransaction(1),
                }
            }
        };
        assert_eq!(
            production,
            expected(
                claim_context,
                Some(ClaimRawTransaction(1)),
                BTreeSet::from([ClaimRawTransaction(1)]),
                true,
                ClaimPoolParent::Other,
            ) == ClaimReleasedInputDisposition::Released
        );
    }
}
