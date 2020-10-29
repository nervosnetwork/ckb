//! TODO(doc): @doitian
use ckb_build_info::Version;
use ckb_resource::{DEFAULT_P2P_PORT, DEFAULT_RPC_PORT, DEFAULT_SPEC};
use clap::{App, AppSettings, Arg, ArgGroup, ArgMatches, SubCommand};

/// TODO(doc): @doitian
pub const CMD_RUN: &str = "run";
/// TODO(doc): @doitian
pub const CMD_MINER: &str = "miner";
/// TODO(doc): @doitian
pub const CMD_EXPORT: &str = "export";
/// TODO(doc): @doitian
pub const CMD_IMPORT: &str = "import";
/// TODO(doc): @doitian
pub const CMD_INIT: &str = "init";
/// TODO(doc): @doitian
pub const CMD_REPLAY: &str = "replay";
/// TODO(doc): @doitian
pub const CMD_STATS: &str = "stats";
/// TODO(doc): @doitian
pub const CMD_LIST_HASHES: &str = "list-hashes";
/// TODO(doc): @doitian
pub const CMD_RESET_DATA: &str = "reset-data";
/// TODO(doc): @doitian
pub const CMD_PEERID: &str = "peer-id";
/// TODO(doc): @doitian
pub const CMD_GEN_SECRET: &str = "gen";
/// TODO(doc): @doitian
pub const CMD_FROM_SECRET: &str = "from-secret";
/// TODO(doc): @doitian
pub const CMD_MIGRATE: &str = "migrate";

/// TODO(doc): @doitian
pub const ARG_CONFIG_DIR: &str = "config-dir";
/// TODO(doc): @doitian
pub const ARG_FORMAT: &str = "format";
/// TODO(doc): @doitian
pub const ARG_TARGET: &str = "target";
/// TODO(doc): @doitian
pub const ARG_SOURCE: &str = "source";
/// TODO(doc): @doitian
pub const ARG_DATA: &str = "data";
/// TODO(doc): @doitian
pub const ARG_LIST_CHAINS: &str = "list-chains";
/// TODO(doc): @doitian
pub const ARG_INTERACTIVE: &str = "interactive";
/// TODO(doc): @doitian
pub const ARG_CHAIN: &str = "chain";
/// TODO(doc): @doitian
pub const ARG_IMPORT_SPEC: &str = "import-spec";
/// TODO(doc): @doitian
pub const ARG_P2P_PORT: &str = "p2p-port";
/// TODO(doc): @doitian
pub const ARG_RPC_PORT: &str = "rpc-port";
/// TODO(doc): @doitian
pub const ARG_FORCE: &str = "force";
/// TODO(doc): @doitian
pub const ARG_LOG_TO: &str = "log-to";
/// TODO(doc): @doitian
pub const ARG_BUNDLED: &str = "bundled";
/// TODO(doc): @doitian
pub const ARG_BA_CODE_HASH: &str = "ba-code-hash";
/// TODO(doc): @doitian
pub const ARG_BA_ARG: &str = "ba-arg";
/// TODO(doc): @doitian
pub const ARG_BA_HASH_TYPE: &str = "ba-hash-type";
/// TODO(doc): @doitian
pub const ARG_BA_MESSAGE: &str = "ba-message";
/// TODO(doc): @doitian
pub const ARG_BA_ADVANCED: &str = "ba-advanced";
/// TODO(doc): @doitian
pub const ARG_FROM: &str = "from";
/// TODO(doc): @doitian
pub const ARG_TO: &str = "to";
/// TODO(doc): @doitian
pub const ARG_ALL: &str = "all";
/// TODO(doc): @doitian
pub const ARG_LIMIT: &str = "limit";
/// TODO(doc): @doitian
pub const ARG_DATABASE: &str = "database";
/// TODO(doc): @doitian
pub const ARG_INDEXER: &str = "indexer";
/// TODO(doc): @doitian
pub const ARG_NETWORK: &str = "network";
/// TODO(doc): @doitian
pub const ARG_NETWORK_PEER_STORE: &str = "network-peer-store";
/// TODO(doc): @doitian
pub const ARG_NETWORK_SECRET_KEY: &str = "network-secret-key";
/// TODO(doc): @doitian
pub const ARG_LOGS: &str = "logs";
/// TODO(doc): @doitian
pub const ARG_TMP_TARGET: &str = "tmp-target";
/// TODO(doc): @doitian
pub const ARG_SECRET_PATH: &str = "secret-path";
/// TODO(doc): @doitian
pub const ARG_PROFILE: &str = "profile";
/// TODO(doc): @doitian
pub const ARG_SANITY_CHECK: &str = "sanity-check";
/// TODO(doc): @doitian
pub const ARG_FULL_VERFICATION: &str = "full-verfication";

/// TODO(doc): @doitian
const GROUP_BA: &str = "ba";

fn basic_app<'b>() -> App<'static, 'b> {
    App::new("ckb")
        .author("Nervos Core Dev <dev@nervos.org>")
        .about("Nervos CKB - The Common Knowledge Base")
        .setting(AppSettings::SubcommandRequiredElseHelp)
        .arg(
            Arg::with_name(ARG_CONFIG_DIR)
                .global(true)
                .short("C")
                .value_name("path")
                .takes_value(true)
                .help(
                    "Runs as if ckb was started in <path> instead of the current working directory.",
                ),
        )
        .subcommand(run())
        .subcommand(miner())
        .subcommand(export())
        .subcommand(import())
        .subcommand(list_hashes())
        .subcommand(init())
        .subcommand(replay())
        .subcommand(stats())
        .subcommand(reset_data())
        .subcommand(peer_id())
        .subcommand(migrate())
}

/// TODO(doc): @doitian
pub fn get_matches(version: &Version) -> ArgMatches<'static> {
    basic_app()
        .version(version.short().as_str())
        .long_version(version.long().as_str())
        .get_matches()
}

fn run() -> App<'static, 'static> {
    SubCommand::with_name(CMD_RUN).about("Runs ckb node").arg(
        Arg::with_name(ARG_BA_ADVANCED)
            .long(ARG_BA_ADVANCED)
            .help("Allows any block assembler code hash and args"),
    )
}

fn miner() -> App<'static, 'static> {
    SubCommand::with_name(CMD_MINER)
        .about("Runs ckb miner")
        .arg(
            Arg::with_name(ARG_LIMIT)
                .short("l")
                .long(ARG_LIMIT)
                .takes_value(true)
                .help(
                    "Exit after how many nonces found; \
            0 means the miner will never exit. [default: 0]",
                ),
        )
}

fn reset_data() -> App<'static, 'static> {
    SubCommand::with_name(CMD_RESET_DATA)
        .about(
            "Truncate the data directory\n\
             Example:\n\
             ckb reset-data --force --indexer",
        )
        .arg(
            Arg::with_name(ARG_FORCE)
                .short("f")
                .long(ARG_FORCE)
                .help("Delete data without interactive prompt"),
        )
        .arg(
            Arg::with_name(ARG_ALL)
                .long(ARG_ALL)
                .help("Delete the whole data directory"),
        )
        .arg(
            Arg::with_name(ARG_DATABASE)
                .long(ARG_DATABASE)
                .help("Delete both `data/db` and `data/indexer_db`"),
        )
        .arg(
            Arg::with_name(ARG_INDEXER)
                .long(ARG_INDEXER)
                .help("Delete only `data/indexer_db`"),
        )
        .arg(
            Arg::with_name(ARG_NETWORK)
                .long(ARG_NETWORK)
                .help("Delete both peer store and secret key"),
        )
        .arg(
            Arg::with_name(ARG_NETWORK_PEER_STORE)
                .long(ARG_NETWORK_PEER_STORE)
                .help("Delete only `data/network/peer_store`"),
        )
        .arg(
            Arg::with_name(ARG_NETWORK_SECRET_KEY)
                .long(ARG_NETWORK_SECRET_KEY)
                .help("Delete only `data/network/secret_key`"),
        )
        .arg(
            Arg::with_name(ARG_LOGS)
                .long(ARG_LOGS)
                .help("Delete only `data/logs`"),
        )
}

pub(crate) fn stats() -> App<'static, 'static> {
    SubCommand::with_name(CMD_STATS)
        .about(
            "Statics chain information\n\
             Example:\n\
             ckb -C <dir> stats --from 1 --to 500",
        )
        .arg(
            Arg::with_name(ARG_FROM)
                .long(ARG_FROM)
                .takes_value(true)
                .help("Specifies from block number."),
        )
        .arg(
            Arg::with_name(ARG_TO)
                .long(ARG_TO)
                .takes_value(true)
                .help("Specifies to block number."),
        )
}

fn replay() -> App<'static, 'static> {
    SubCommand::with_name(CMD_REPLAY)
        .about("replay ckb process block")
        .help("
            --tmp-target <tmp> --profile 1 10,\n
            --tmp-target <tmp> --sanity-check,\n
        ")
        .arg(Arg::with_name(ARG_TMP_TARGET).long(ARG_TMP_TARGET).takes_value(true).required(true).help(
            "Specifies a target path, prof command make a temporary directory inside of target and the directory will be automatically deleted when finished",
        ))
        .arg(Arg::with_name(ARG_PROFILE).long(ARG_PROFILE).help(
            "Enable profile",
        ))
        .arg(
            Arg::with_name(ARG_FROM)
              .help("Specifies profile from block number."),
        )
        .arg(
            Arg::with_name(ARG_TO)
              .help("Specifies profile to block number."),
        )
        .arg(
            Arg::with_name(ARG_SANITY_CHECK).long(ARG_SANITY_CHECK).help("Enable sanity check")
        )
        .arg(
            Arg::with_name(ARG_FULL_VERFICATION).long(ARG_FULL_VERFICATION).help("Enable sanity check")
        )
        .group(
            ArgGroup::with_name("mode")
                .args(&[ARG_PROFILE, ARG_SANITY_CHECK])
                .required(true)
        )
}

fn export() -> App<'static, 'static> {
    SubCommand::with_name(CMD_EXPORT)
        .about("Exports ckb data")
        .arg(
            Arg::with_name(ARG_TARGET)
                .short("t")
                .long(ARG_TARGET)
                .value_name("path")
                .required(true)
                .index(1)
                .help("Specifies the export target path."),
        )
}

fn import() -> App<'static, 'static> {
    SubCommand::with_name(CMD_IMPORT)
        .about("Imports ckb data")
        .arg(
            Arg::with_name(ARG_SOURCE)
                .short("s")
                .long(ARG_SOURCE)
                .value_name("path")
                .required(true)
                .index(1)
                .help("Specifies the exported data path."),
        )
}

fn migrate() -> App<'static, 'static> {
    SubCommand::with_name(CMD_MIGRATE).about("Runs ckb migration")
}

fn list_hashes() -> App<'static, 'static> {
    SubCommand::with_name(CMD_LIST_HASHES)
        .about("Lists well known hashes")
        .arg(
            Arg::with_name(ARG_BUNDLED)
                .short("b")
                .long(ARG_BUNDLED)
                .help(
                    "Lists hashes of the bundled chain specs instead of the current effective one.",
                ),
        )
}

fn init() -> App<'static, 'static> {
    SubCommand::with_name(CMD_INIT)
        .about("Creates a CKB direcotry or reinitializes an existing one")
        .arg(
            Arg::with_name(ARG_INTERACTIVE)
                .short("i")
                .long(ARG_INTERACTIVE)
                .help("Interactive mode"),
        )
        .arg(
            Arg::with_name(ARG_LIST_CHAINS)
                .short("l")
                .long(ARG_LIST_CHAINS)
                .help("Lists available options for --chain"),
        )
        .arg(
            Arg::with_name(ARG_CHAIN)
                .short("c")
                .long(ARG_CHAIN)
                .default_value(DEFAULT_SPEC)
                .help("Initializes CKB direcotry for <chain>"),
        )
        .arg(
            Arg::with_name(ARG_IMPORT_SPEC)
                .long(ARG_IMPORT_SPEC)
                .takes_value(true)
                .help(
                    "Uses the specifiec file as chain spec. Specially, \
                     The dash \"-\" denotes importing the spec from stdin encoded in base64",
                ),
        )
        .arg(
            Arg::with_name(ARG_LOG_TO)
                .long(ARG_LOG_TO)
                .possible_values(&["file", "stdout", "both"])
                .default_value("both")
                .help("Configures where the logs should print"),
        )
        .arg(
            Arg::with_name(ARG_FORCE)
                .short("f")
                .long(ARG_FORCE)
                .help("Forces overwriting existing files"),
        )
        .arg(
            Arg::with_name(ARG_RPC_PORT)
                .long(ARG_RPC_PORT)
                .default_value(DEFAULT_RPC_PORT)
                .help("Replaces CKB RPC port in the created config file"),
        )
        .arg(
            Arg::with_name(ARG_P2P_PORT)
                .long(ARG_P2P_PORT)
                .default_value(DEFAULT_P2P_PORT)
                .help("Replaces CKB P2P port in the created config file"),
        )
        .arg(
            Arg::with_name(ARG_BA_CODE_HASH)
                .long(ARG_BA_CODE_HASH)
                .value_name("code_hash")
                .validator(is_hex)
                .takes_value(true)
                .help(
                    "Sets code_hash in [block_assembler] \
                     [default: secp256k1 if --ba-arg is present]",
                ),
        )
        .arg(
            Arg::with_name(ARG_BA_ARG)
                .long(ARG_BA_ARG)
                .value_name("arg")
                .validator(is_hex)
                .multiple(true)
                .number_of_values(1)
                .help("Sets args in [block_assembler]"),
        )
        .arg(
            Arg::with_name(ARG_BA_HASH_TYPE)
                .long(ARG_BA_HASH_TYPE)
                .value_name("hash_type")
                .takes_value(true)
                .possible_values(&["data", "type"])
                .default_value("type")
                .help("Sets hash type in [block_assembler]"),
        )
        .group(
            ArgGroup::with_name(GROUP_BA)
                .args(&[ARG_BA_CODE_HASH, ARG_BA_ARG])
                .multiple(true),
        )
        .arg(
            Arg::with_name(ARG_BA_MESSAGE)
                .long(ARG_BA_MESSAGE)
                .value_name("message")
                .validator(is_hex)
                .requires(GROUP_BA)
                .help("Sets message in [block_assembler]"),
        )
        .arg(
            Arg::with_name("export-specs")
                .long("export-specs")
                .hidden(true),
        )
        .arg(Arg::with_name("list-specs").long("list-specs").hidden(true))
        .arg(
            Arg::with_name("spec")
                .short("s")
                .long("spec")
                .takes_value(true)
                .hidden(true),
        )
}

fn peer_id() -> App<'static, 'static> {
    SubCommand::with_name(CMD_PEERID)
        .about("About peer id, base on Secp256k1")
        .subcommand(
            SubCommand::with_name(CMD_FROM_SECRET)
                .about("Generate peer id from secret file")
                .arg(
                    Arg::with_name(ARG_SECRET_PATH)
                        .takes_value(true)
                        .long(ARG_SECRET_PATH)
                        .required(true)
                        .help("Generate peer id from secret file path"),
                ),
        )
        .subcommand(
            SubCommand::with_name(CMD_GEN_SECRET)
                .about("Generate random key to file")
                .arg(
                    Arg::with_name(ARG_SECRET_PATH)
                        .long(ARG_SECRET_PATH)
                        .required(true)
                        .takes_value(true)
                        .help("Generate peer id to file path"),
                ),
        )
}

fn is_hex(hex: String) -> Result<(), String> {
    let tmp = hex.as_bytes();
    if tmp.len() < 2 {
        Err("Must be a 0x-prefixed hexadecimal string".to_string())
    } else if tmp.len() & 1 != 0 {
        Err("Hexadecimal strings must be of even length".to_string())
    } else if tmp[..2] == b"0x"[..] {
        for byte in &tmp[2..] {
            match byte {
                b'A'..=b'F' | b'a'..=b'f' | b'0'..=b'9' => continue,
                invalid_char => {
                    return Err(format!("Hex has invalid char: {}", invalid_char));
                }
            }
        }

        Ok(())
    } else {
        Err("Must 0x-prefixed hexadecimal string".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ba_message_requires_ba_arg_or_ba_code_hash() {
        let ok_ba_arg = basic_app().get_matches_from_safe(&[
            "ckb",
            "init",
            "--ba-message",
            "0x00",
            "--ba-arg",
            "0x00",
        ]);
        let ok_ba_code_hash = basic_app().get_matches_from_safe(&[
            "ckb",
            "init",
            "--ba-message",
            "0x00",
            "--ba-code-hash",
            "0x00",
        ]);
        let err = basic_app().get_matches_from_safe(&["ckb", "init", "--ba-message", "0x00"]);

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
            "ckb",
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
            .get_matches_from_safe(&["ckb", "run", "--ba-advanced"])
            .unwrap();
        let sub_matches = matches.subcommand().1.unwrap();

        assert_eq!(1, sub_matches.occurrences_of(ARG_BA_ADVANCED));
    }
}
