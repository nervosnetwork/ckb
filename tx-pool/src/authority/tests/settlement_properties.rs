//! Direct regression for settlement precedence after dependency-baseline loss.

use super::foundation::{apply_plan, limits, owner_version, take_resolve_work};
use crate::authority::{
    plan::TxPoolAuthority,
    state::{OwnedTx, PreAcceptedPhase, QueuedWork, ValidatedAdmission, WorkPermit},
};
use ckb_network::PeerIndex;
use ckb_types::{
    bytes::Bytes,
    core::TransactionBuilder,
    packed::{CellInput, CellOutput, OutPoint},
    prelude::Pack,
};

#[test]
fn uak_baseline_loss_dominates_a_resource_bound_settlement() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let parent_tx = TransactionBuilder::default()
        .version(41_001u32)
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .build();
    let parent_admission = ValidatedAdmission::proposal(parent_tx.clone()).expect("valid parent");
    let parent = parent_admission.identity.raw.clone();
    apply_plan(
        authority
            .plan_admission(parent_admission)
            .expect("parent admission plans"),
    );

    let child_tx = TransactionBuilder::default()
        .version(41_002u32)
        .input(CellInput::new(OutPoint::new(parent_tx.hash(), 0), 0))
        .build();
    let child_admission =
        ValidatedAdmission::remote(child_tx, PeerIndex::from(41_002)).expect("valid child");
    let child = child_admission.identity.raw.clone();
    apply_plan(
        authority
            .plan_admission(child_admission)
            .expect("child admission plans"),
    );
    let (_, work) = take_resolve_work(
        authority
            .checkout_for_foundation(
                &child,
                owner_version(&authority, &child),
                WorkPermit::ResolveOnly,
            )
            .expect("child Resolve checks out"),
    );
    let resource_rejection = work.resource_denied();

    apply_plan(
        authority
            .plan_terminalize_for_foundation(&parent, owner_version(&authority, &parent))
            .expect("parent loss terminalizes"),
    );
    apply_plan(
        authority
            .apply_settlement(resource_rejection)
            .expect("active child settles after loss"),
    );
    assert!(matches!(
        authority.entry(&child),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Resolve))
    ));
}
