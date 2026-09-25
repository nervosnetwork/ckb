use crate::{
    TxVerifyEnv,
    cache::{ScriptVerificationRules, TxVerificationCacheKey},
};
use ckb_chain_spec::consensus::{Consensus, ConsensusBuilder};
use ckb_script::{ScriptError, ScriptVersion, TxData};
use ckb_traits::CellDataProvider;
use ckb_types::{
    bytes::Bytes,
    core::{
        EpochNumberWithFraction, HeaderBuilder, ScriptHashType, TransactionBuilder,
        cell::ResolvedTransaction,
        hardfork::{CKB2021, CKB2023, HardForks},
    },
    packed::{Byte32, OutPoint, Script},
    prelude::{Builder, Entity, Pack, Unpack},
};
use std::sync::Arc;

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

struct EmptyCells;

impl CellDataProvider for EmptyCells {
    fn get_cell_data(&self, _: &OutPoint) -> Option<Bytes> {
        None
    }

    fn get_cell_data_hash(&self, _: &OutPoint) -> Option<Byte32> {
        None
    }
}

fn script_context(
    v1_epoch: u64,
    v2_epoch: u64,
    environment: TxVerifyEnv,
) -> (ScriptVerificationRules, TxData<EmptyCells>) {
    let consensus: Arc<Consensus> = Arc::new(
        ConsensusBuilder::default()
            .hardfork_switch(HardForks {
                ckb2021: CKB2021::new_dev_default()
                    .as_builder()
                    .rfc_0032(v1_epoch)
                    .build()
                    .unwrap(),
                ckb2023: CKB2023::new_with_specified(v2_epoch),
            })
            .build(),
    );
    let rules = ScriptVerificationRules::from_env(&consensus, &environment);
    let context = TxData::new(
        Arc::new(ResolvedTransaction::dummy_resolve(
            TransactionBuilder::default().build(),
        )),
        EmptyCells,
        consensus,
        Arc::new(environment),
    );
    (rules, context)
}

#[test]
fn data_script_gates_remain_independent_of_the_latest_active_vm() {
    use ScriptVerificationRules::{V0, V1, V2};
    let header = HeaderBuilder::default()
        .epoch(EpochNumberWithFraction::new(0, 0, 10))
        .build();
    // ConsensusBuilder can express independently enabled gates, even though
    // node configurations activate VM1 no later than VM2.
    for (v1, v2, expected_rules, expected_type) in [
        (false, false, V0, ScriptVersion::V0),
        (true, false, V1, ScriptVersion::V1),
        (false, true, V2, ScriptVersion::V2),
        (true, true, V2, ScriptVersion::V2),
    ] {
        let activation = |enabled| if enabled { 0 } else { u64::MAX };
        let (rules, context) = script_context(
            activation(v1),
            activation(v2),
            TxVerifyEnv::new_commit(&header),
        );
        let select =
            |hash_type| context.select_version(&Script::new_builder().hash_type(hash_type).build());
        assert_eq!(rules, expected_rules);
        assert_eq!(select(ScriptHashType::Data), Ok(ScriptVersion::V0));
        assert_eq!(select(ScriptHashType::Type), Ok(expected_type));
        assert_eq!(
            select(ScriptHashType::Data1),
            if v1 {
                Ok(ScriptVersion::V1)
            } else {
                Err(ScriptError::InvalidVmVersion(1))
            }
        );
        assert_eq!(
            select(ScriptHashType::Data2),
            if v2 {
                Ok(ScriptVersion::V2)
            } else {
                Err(ScriptError::InvalidVmVersion(2))
            }
        );
    }
}

#[test]
fn cached_rules_and_execution_do_not_activate_vms_through_the_proposal_window() {
    use ScriptVerificationRules::{V0, V1, V2};
    for (epoch, index, committed, inflight) in [
        (4, 8, (V0, ScriptVersion::V0), (V0, ScriptVersion::V0)),
        (4, 9, (V0, ScriptVersion::V0), (V1, ScriptVersion::V1)),
        (5, 0, (V1, ScriptVersion::V1), (V1, ScriptVersion::V1)),
        (9, 8, (V1, ScriptVersion::V1), (V1, ScriptVersion::V1)),
        (9, 9, (V1, ScriptVersion::V1), (V2, ScriptVersion::V2)),
        (10, 0, (V2, ScriptVersion::V2), (V2, ScriptVersion::V2)),
    ] {
        let header = HeaderBuilder::default()
            .number(epoch * 10 + index)
            .epoch(EpochNumberWithFraction::new(epoch, index, 10))
            .build();
        for (environment, expected) in [
            (TxVerifyEnv::new_commit(&header), committed),
            (TxVerifyEnv::new_submit(&header), inflight),
            (TxVerifyEnv::new_proposed(&header, 0), inflight),
            (TxVerifyEnv::new_proposed(&header, 1), inflight),
        ] {
            let (rules, context) = script_context(5, 10, environment);
            let selected = context
                .select_version(
                    &Script::new_builder()
                        .hash_type(ScriptHashType::Type)
                        .build(),
                )
                .unwrap();
            assert_eq!((rules, selected), expected, "epoch {epoch}, index {index}");
        }
    }
}
