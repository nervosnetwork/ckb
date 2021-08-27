use std::fs::read;
use std::path::Path;

use crate::{
    ill_transaction_checker::{IllScriptChecker, CKB_VM_ISSUE_92},
    ScriptError,
};

#[test]
fn check_good_binary() {
    let data =
        read(Path::new(env!("CARGO_MANIFEST_DIR")).join("../script/testdata/verify")).unwrap();
    assert!(IllScriptChecker::new(&data, 13).check().is_ok());
}

#[test]
fn check_defected_binary() {
    let data =
        read(Path::new(env!("CARGO_MANIFEST_DIR")).join("../script/testdata/defected_binary"))
            .unwrap();
    assert_eq!(
        IllScriptChecker::new(&data, 13).check().unwrap_err(),
        ScriptError::EncounteredKnownBugs(CKB_VM_ISSUE_92.to_string(), 13),
    );
}

#[test]
fn check_jalr_zero_binary() {
    let data =
        read(Path::new(env!("CARGO_MANIFEST_DIR")).join("../script/testdata/jalr_zero")).unwrap();
    assert!(IllScriptChecker::new(&data, 13).check().is_ok());
}
