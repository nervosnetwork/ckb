use clap::Command;

use crate::cli::*;

#[test]
fn stats_args() {
    let app = Command::new("stats_args_test")
        .arg_required_else_help(true)
        .subcommand(stats());

    let stats = app.clone().try_get_matches_from(vec!["", CMD_STATS]);
    assert!(stats.is_ok());

    let stats = app
        .clone()
        .try_get_matches_from(vec!["", CMD_STATS, "--from", "10"]);
    assert!(stats.is_ok());

    let stats = app
        .clone()
        .try_get_matches_from(vec!["", CMD_STATS, "--to", "100"]);
    assert!(stats.is_ok());

    let stats = app
        .clone()
        .try_get_matches_from(vec!["", CMD_STATS, "--from", "10", "--to", "100"]);
    assert!(stats.is_ok());
}

#[test]
fn ba_message_requires_ba_arg_or_ba_code_hash() {
    let ok_ba_arg = basic_app().try_get_matches_from(&[
        BIN_NAME,
        "init",
        "--ba-message",
        "0x00",
        "--ba-arg",
        "0x00",
    ]);
    let ba_code_hash = basic_app().try_get_matches_from(&[
        BIN_NAME,
        "init",
        "--ba-message",
        "0x00",
        "--ba-code-hash",
        "0x00",
    ]);
    let err = basic_app().try_get_matches_from(&[BIN_NAME, "init", "--ba-message", "0x00"]);

    assert!(
        ok_ba_arg.is_ok(),
        "--ba-message is ok with --ba-arg, but gets error: {:?}",
        ok_ba_arg.err()
    );
    assert!(ba_code_hash.is_err());
    assert!(
        err.is_err(),
        "--ba-message requires --ba-arg or --ba-code-hash"
    );

    let err = err.err().unwrap();
    assert_eq!(clap::ErrorKind::MissingRequiredArgument, err.kind());
    assert!(err
        .to_string()
        .contains("The following required arguments were not provided"));
    assert!(err.to_string().contains("--ba-arg"));
    assert!(err.to_string().contains("--ba-code-hash"));
}

#[test]
fn ba_arg_and_ba_code_hash() {
    let matches = basic_app().try_get_matches_from(&[
        BIN_NAME,
        "init",
        "--ba-code-hash",
        "0x00",
        "--ba-arg",
        "0x00",
    ]);
    assert!(matches.is_err());
}

#[test]
fn ba_advanced() {
    let matches = basic_app()
        .try_get_matches_from(&[BIN_NAME, "run", "--ba-advanced"])
        .unwrap();
    let sub_matches = matches.subcommand().unwrap().1;

    assert_eq!(1, sub_matches.occurrences_of(ARG_BA_ADVANCED));
}

#[test]
/// 2 cases in which use h256 validator:
/// ckb init --ba-code-hash
/// ckb run --assume-valid-target
/// not for `ckb init --ba-arg` && `ckb init --ba-message`
fn h256_as_validator() {
    let ok_matches = basic_app().try_get_matches_from(&[
        BIN_NAME,
        "init",
        "--ba-code-hash",
        "0x00d1b86f6824d33a91b72ec20e2118cf7788a5ffff656bd1ea1ea638c764cb5f",
        "--ba-arg",
        "0x00",
    ]);
    assert!(ok_matches.is_ok());

    let err_matches = basic_app().try_get_matches_from(&[
        BIN_NAME,
        "init",
        "--ba-code-hash",
        "0xd1b86f6824d33a91b72ec20e2118cf7788a5ffff656bd1ea1ea638c764cb5f",
        "--ba-arg",
        "0x00",
    ]);
    let err = err_matches.err().unwrap();
    assert_eq!(clap::ErrorKind::ValueValidation, err.kind());

    let err_matches = basic_app().try_get_matches_from(&[
        BIN_NAME,
        "init",
        "--ba-code-hash",
        "0x4630c0",
        "--ba-arg",
        "0x00",
    ]);
    let err = err_matches.err().unwrap();
    assert_eq!(clap::ErrorKind::ValueValidation, err.kind());

    let ok_matches = basic_app().try_get_matches_from(&[
        BIN_NAME,
        "run",
        "--assume-valid-target",
        "0x94a4e93601f7295501891764880d37e9fcf886d02bf64b3d06f9137db8fa981e",
    ]);
    assert!(ok_matches.is_ok());
    let err_matches = basic_app().try_get_matches_from(&[
        BIN_NAME,
        "run",
        "--assume-valid-target",
        "0x94a4e93601f729550",
    ]);
    let err = err_matches.err().unwrap();
    assert_eq!(clap::ErrorKind::ValueValidation, err.kind());
}
