use clap::{App, AppSettings};

use crate::cli::*;

#[test]
fn stats_args() {
    let app = App::new("stats_args_test")
        .setting(AppSettings::SubcommandRequiredElseHelp)
        .subcommand(stats());

    let stats = app.clone().get_matches_from_safe(vec!["", CMD_STATS]);
    assert!(stats.is_ok());

    let stats = app
        .clone()
        .get_matches_from_safe(vec!["", CMD_STATS, "--from", "10"]);
    assert!(stats.is_ok());

    let stats = app
        .clone()
        .get_matches_from_safe(vec!["", CMD_STATS, "--to", "100"]);
    assert!(stats.is_ok());

    let stats = app
        .clone()
        .get_matches_from_safe(vec!["", CMD_STATS, "--from", "10", "--to", "100"]);
    assert!(stats.is_ok());
}

#[test]
fn ba_message_requires_ba_arg_or_ba_code_hash() {
    let ok_ba_arg = basic_app().get_matches_from_safe(&[
        BIN_NAME,
        "init",
        "--ba-message",
        "0x00",
        "--ba-arg",
        "0x00",
    ]);
    let ok_ba_code_hash = basic_app().get_matches_from_safe(&[
        BIN_NAME,
        "init",
        "--ba-message",
        "0x00",
        "--ba-code-hash",
        "0x00",
    ]);
    let err = basic_app().get_matches_from_safe(&[BIN_NAME, "init", "--ba-message", "0x00"]);

    assert!(
        ok_ba_arg.is_ok(),
        "--ba-message is ok with --ba-arg, but gets error: {:?}",
        ok_ba_arg.err()
    );
    assert!(
        ok_ba_code_hash.is_ok(),
        "--ba-message is ok with --ba-code-hash, but gets error: {:?}",
        ok_ba_code_hash.err()
    );
    assert!(
        err.is_err(),
        "--ba-message requires --ba-arg or --ba-code-hash"
    );

    let err = err.err().unwrap();
    assert_eq!(clap::ErrorKind::MissingRequiredArgument, err.kind);
    assert!(err
        .message
        .contains("The following required arguments were not provided"));
    assert!(err.message.contains("--ba-arg"));
    assert!(err.message.contains("--ba-code-hash"));
}

#[test]
fn ba_arg_and_ba_code_hash() {
    let ok_matches = basic_app().get_matches_from_safe(&[
        BIN_NAME,
        "init",
        "--ba-code-hash",
        "0x00",
        "--ba-arg",
        "0x00",
    ]);
    assert!(
        ok_matches.is_ok(),
        "--ba-code-hash is OK with --ba-arg, but gets error: {:?}",
        ok_matches.err()
    );
}

#[test]
fn ba_advanced() {
    let matches = basic_app()
        .get_matches_from_safe(&[BIN_NAME, "run", "--ba-advanced"])
        .unwrap();
    let sub_matches = matches.subcommand().1.unwrap();

    assert_eq!(1, sub_matches.occurrences_of(ARG_BA_ADVANCED));
}
