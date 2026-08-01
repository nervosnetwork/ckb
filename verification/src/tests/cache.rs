use crate::{
    TxVerifyEnv,
    cache::{ScriptVerificationRules, TxVerificationCacheKey},
};
use ckb_chain_spec::consensus::ConsensusBuilder;
use ckb_types::{
    bytes::Bytes,
    core::{EpochNumberWithFraction, HeaderBuilder, TransactionBuilder, hardfork::HardForks},
    prelude::{Pack, Unpack},
};

#[test]
fn cache_key_binds_witness_identity_to_script_rules() {
    let tx = TransactionBuilder::default()
        .witness(Bytes::from_static(b"witness").pack())
        .build();
    let expected_witness_hash: [u8; 32] = tx.witness_hash().unpack();
    let v1 = TxVerificationCacheKey::from_transaction(&tx, ScriptVerificationRules::V1);
    let v2 = TxVerificationCacheKey::from_transaction(&tx, ScriptVerificationRules::V2);

    assert_eq!(v1.witness_hash(), &expected_witness_hash);
    assert_eq!(v1.script_rules(), ScriptVerificationRules::V1);
    assert_ne!(v1, v2);
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
