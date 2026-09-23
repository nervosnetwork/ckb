use crate::{
    TxVerifyEnv,
    cache::{ScriptVerificationRules, TxVerificationCacheKey},
};
use ckb_chain_spec::consensus::ConsensusBuilder;
use ckb_types::{
    bytes::Bytes,
    core::{
        EpochNumberWithFraction, HeaderBuilder, TransactionBuilder, cell::ResolvedTransaction,
        hardfork::HardForks,
    },
    prelude::{Pack, Unpack},
};

#[test]
fn cache_key_binds_witness_identity_to_script_rules() {
    let tx = TransactionBuilder::default()
        .witness(Bytes::from_static(b"witness").pack())
        .build();
    let expected_witness_hash: [u8; 32] = tx.witness_hash().unpack();
    let resolved = ResolvedTransaction::dummy_resolve(tx);
    let v1 = TxVerificationCacheKey::from_resolved(&resolved, ScriptVerificationRules::V1);
    let v2 = TxVerificationCacheKey::from_resolved(&resolved, ScriptVerificationRules::V2);

    assert_eq!(v1.witness_hash(), &expected_witness_hash);
    assert_eq!(v1.script_rules(), ScriptVerificationRules::V1);
    assert_ne!(v1, v2);
}

#[test]
fn cache_key_distinguishes_witnesses_for_the_same_raw_transaction() {
    let tx = TransactionBuilder::default()
        .witness(Bytes::from_static(b"first").pack())
        .build();
    let cousin = tx
        .as_advanced_builder()
        .set_witnesses(vec![Bytes::from_static(b"second").pack()])
        .build();
    assert_eq!(tx.hash(), cousin.hash());
    assert_ne!(tx.witness_hash(), cousin.witness_hash());
    assert_ne!(
        TxVerificationCacheKey::from_resolved(
            &ResolvedTransaction::dummy_resolve(tx),
            ScriptVerificationRules::V1
        ),
        TxVerificationCacheKey::from_resolved(
            &ResolvedTransaction::dummy_resolve(cousin),
            ScriptVerificationRules::V1
        ),
    );
}

#[test]
fn cache_key_tracks_script_visible_cell_origins_in_input_and_dep_order() {
    use ckb_types::{
        core::{TransactionInfo, cell::CellMeta},
        packed::Byte32,
    };

    let header = HeaderBuilder::default()
        .number(1u64)
        .epoch(EpochNumberWithFraction::new(0, 1, 10))
        .build();
    let other = HeaderBuilder::default()
        .number(2u64)
        .epoch(EpochNumberWithFraction::new(0, 2, 10))
        .build();
    let cell = |hash: Option<Byte32>| CellMeta {
        transaction_info: hash.map(|block_hash| TransactionInfo {
            block_hash,
            block_number: 1,
            block_epoch: EpochNumberWithFraction::new(0, 0, 1),
            index: 1,
        }),
        ..Default::default()
    };
    let mut resolved = ResolvedTransaction::dummy_resolve(
        TransactionBuilder::default()
            .header_dep(header.hash())
            .build(),
    );
    resolved.resolved_inputs = vec![cell(None), cell(None)];
    resolved.resolved_cell_deps = vec![cell(None)];
    let key = |rtx: &ResolvedTransaction| {
        TxVerificationCacheKey::from_resolved(rtx, ScriptVerificationRules::V2)
    };
    let missing = key(&resolved);
    // A block outside header_deps stays invisible to both origin syscalls.
    resolved.resolved_inputs[0] = cell(Some(other.hash()));
    assert_eq!(key(&resolved), missing);
    resolved.resolved_inputs[0] = cell(Some(header.hash()));
    let first = key(&resolved);
    assert_ne!(first, missing);
    resolved.resolved_inputs.swap(0, 1);
    assert_ne!(key(&resolved), first);
    assert_ne!(key(&resolved), missing);
    resolved.resolved_inputs[1] = cell(None);
    resolved.resolved_cell_deps[0] = cell(Some(header.hash()));
    assert_ne!(key(&resolved), first);
    assert_ne!(key(&resolved), missing);
    let expanded_member = key(&resolved);
    // The group container has no syscall index; its expanded member above does.
    resolved.resolved_dep_groups = vec![cell(Some(other.hash()))];
    assert_eq!(key(&resolved), expanded_member);
    resolved.resolved_dep_groups[0] = cell(Some(header.hash()));
    assert_eq!(key(&resolved), expanded_member);

    resolved.transaction = TransactionBuilder::default()
        .header_dep(header.hash())
        .header_dep(other.hash())
        .build();
    let first_origin = key(&resolved);
    resolved.resolved_cell_deps[0] = cell(Some(other.hash()));
    assert_ne!(
        key(&resolved),
        first_origin,
        "both origins are visible but different"
    );

    // With no header deps, all origins are unobservable and can share proof.
    resolved.transaction = TransactionBuilder::default().build();
    let no_headers = key(&resolved);
    resolved.resolved_cell_deps[0] = cell(None);
    assert_eq!(key(&resolved), no_headers);
}

#[test]
fn verification_rules_follow_the_tx_environment_hardfork_boundary() {
    let hardforks = HardForks::new_mirana();
    let v1_epoch = hardforks.ckb2021.vm_version_1_and_syscalls_2();
    let v2_epoch = hardforks.ckb2023.vm_version_2_and_syscalls_3();
    let consensus = ConsensusBuilder::default()
        .hardfork_switch(hardforks)
        .build();
    let rules_at = |epoch| {
        let header = HeaderBuilder::default()
            .epoch(EpochNumberWithFraction::new(epoch, 0, 1))
            .build();
        ScriptVerificationRules::from_env(&consensus, &TxVerifyEnv::new_commit(&header))
    };

    assert_eq!(
        rules_at(v1_epoch.saturating_sub(1)),
        ScriptVerificationRules::V0
    );
    assert_eq!(rules_at(v1_epoch), ScriptVerificationRules::V1);
    assert_eq!(rules_at(v2_epoch), ScriptVerificationRules::V2);
}
