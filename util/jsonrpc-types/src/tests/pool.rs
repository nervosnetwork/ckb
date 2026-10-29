use crate::pool::{OutputsValidator, PoolTransactionReject};
use ckb_types::core::tx_pool::Reject;

#[test]
fn public_rejection_preserves_pool_full_and_excludes_internal_outcomes() {
    let full = Reject::Full("capacity".to_owned());
    let public = PoolTransactionReject::try_from(full).unwrap();
    assert_eq!(
        serde_json::to_value(public).unwrap(),
        serde_json::json!({
            "type": "Full",
            "description": "Transaction is replaced because the pool is full, capacity",
        }),
    );
    assert!(matches!(
        PoolTransactionReject::try_from(Reject::ExcessiveVerifyTime),
        Err(Reject::ExcessiveVerifyTime)
    ));
}

#[test]
fn test_outputs_validator_json_display() {
    assert_eq!(
        "well_known_scripts_only",
        OutputsValidator::WellKnownScriptsOnly.json_display()
    );
    assert_eq!("passthrough", OutputsValidator::Passthrough.json_display());
}
