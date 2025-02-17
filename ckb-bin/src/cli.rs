//! CKB command line arguments parser.
use ckb_build_info::Version;
use ckb_resource::{AVAILABLE_SPECS, DEFAULT_P2P_PORT, DEFAULT_RPC_PORT, DEFAULT_SPEC};
use clap::{Arg, ArgGroup, ArgMatches, Command};

pub use ckb_app_config::cli::*;

/// Subcommand `export`.
pub const CMD_EXPORT: &str = "export";
/// Subcommand `import`.
pub const CMD_IMPORT: &str = "import";
/// Subcommand `init`.
pub const CMD_INIT: &str = "init";
/// Subcommand `replay`.
pub const CMD_REPLAY: &str = "replay";
/// Subcommand `stats`.
pub const CMD_STATS: &str = "stats";
/// Subcommand `list-hashes`.
pub const CMD_LIST_HASHES: &str = "list-hashes";
/// Subcommand `peer-id`.
pub const CMD_PEERID: &str = "peer-id";
/// Subcommand `gen`.
pub const CMD_GEN_SECRET: &str = "gen";
/// Subcommand `from-secret`.
pub const CMD_FROM_SECRET: &str = "from-secret";
/// Subcommand `migrate`.
pub const CMD_MIGRATE: &str = "migrate";
/// Subcommand `daemon`
pub const CMD_DAEMON: &str = "daemon";
/// Command line argument `--config-dir`.
pub const ARG_CONFIG_DIR: &str = "config-dir";
/// Command line argument `--format`.
pub const ARG_FORMAT: &str = "format";
/// Command line argument `--target`.
pub const ARG_TARGET: &str = "target";
/// Command line argument `--source`.
pub const ARG_SOURCE: &str = "source";
/// Command line argument `--data`.
pub const ARG_DATA: &str = "data";
/// Command line argument `--list-chains`.
pub const ARG_LIST_CHAINS: &str = "list-chains";
/// Command line argument `--interactive`.
pub const ARG_INTERACTIVE: &str = "interactive";
/// Command line argument `--chain`.
pub const ARG_CHAIN: &str = "chain";
/// Command line argument `--import-spec`.
pub const ARG_IMPORT_SPEC: &str = "import-spec";
/// The argument for the genesis message.
pub const ARG_GENESIS_MESSAGE: &str = "genesis-message";
/// Command line argument `--p2p-port`.
pub const ARG_P2P_PORT: &str = "p2p-port";
/// Command line argument `--rpc-port`.
pub const ARG_RPC_PORT: &str = "rpc-port";
/// Command line argument `--force`.
pub const ARG_FORCE: &str = "force";
/// Command line argument `--include-background`.
pub const ARG_INCLUDE_BACKGROUND: &str = "include-background";
/// Command line argument `--log-to`.
pub const ARG_LOG_TO: &str = "log-to";
/// Command line argument `--bundled`.
pub const ARG_BUNDLED: &str = "bundled";
/// Command line argument `--ba-code-hash`.
pub const ARG_BA_CODE_HASH: &str = "ba-code-hash";
/// Command line argument `--ba-arg`.
pub const ARG_BA_ARG: &str = "ba-arg";
/// Command line argument `--ba-hash-type`.
pub const ARG_BA_HASH_TYPE: &str = "ba-hash-type";
/// Command line argument `--ba-message`.
pub const ARG_BA_MESSAGE: &str = "ba-message";
/// Command line argument `--ba-advanced`.
pub const ARG_BA_ADVANCED: &str = "ba-advanced";
/// Command line argument `--daemon`
pub const ARG_DAEMON: &str = "daemon";
/// Command line argument `--indexer`.
pub const ARG_INDEXER: &str = "indexer";
/// Command line argument `--rich-indexer`.
pub const ARG_RICH_INDEXER: &str = "rich-indexer";
/// Command line argument `--from`.
pub const ARG_FROM: &str = "from";
/// Command line argument `--to`.
pub const ARG_TO: &str = "to";
/// Command line argument `--all`.
pub const ARG_ALL: &str = "all";
/// Command line argument `--limit`.
pub const ARG_LIMIT: &str = "limit";
/// Command line argument `--database`.
pub const ARG_DATABASE: &str = "database";
/// Command line argument `--network`.
pub const ARG_NETWORK: &str = "network";
/// Command line argument `--network-peer-store`.
pub const ARG_NETWORK_PEER_STORE: &str = "network-peer-store";
/// Command line argument `--network-secret-key`.
pub const ARG_NETWORK_SECRET_KEY: &str = "network-secret-key";
/// Command line argument `--logs`.
pub const ARG_LOGS: &str = "logs";
/// Command line argument `--tmp-target`.
pub const ARG_TMP_TARGET: &str = "tmp-target";
/// Command line argument `--secret-path`.
pub const ARG_SECRET_PATH: &str = "secret-path";
/// Command line argument `--profile`.
pub const ARG_PROFILE: &str = "profile";
/// Command line argument `--sanity-check`.
pub const ARG_SANITY_CHECK: &str = "sanity-check";
/// Command line argument `--full-verification`.
pub const ARG_FULL_VERIFICATION: &str = "full-verification";
/// Command line argument `--skip-spec-check`.
pub const ARG_SKIP_CHAIN_SPEC_CHECK: &str = "skip-spec-check";
/// Present `overwrite-spec` arg to force overriding the chain spec in the database with the present configured chain spec
pub const ARG_OVERWRITE_CHAIN_SPEC: &str = "overwrite-spec";
/// Command line argument `--assume-valid-target`.
pub const ARG_ASSUME_VALID_TARGET: &str = "assume-valid-target";
/// Command line argument `--check`.
pub const ARG_MIGRATE_CHECK: &str = "check";
/// Command line argument `daemon --check`
pub const ARG_DAEMON_CHECK: &str = "check";
/// Command line argument `daemon --stop`
pub const ARG_DAEMON_STOP: &str = "stop";

/// Command line arguments group `ba` for block assembler.
const GROUP_BA: &str = "ba";

/// return root clap Command
pub fn basic_app() -> Command {
    let command = Command::new(BIN_NAME)
        .author("Nervos Core Dev <dev@nervos.org>")
        .about("Nervos CKB - The Common Knowledge Base")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .term_width(110)
        .arg(
            Arg::new(ARG_CONFIG_DIR)
                .global(true)
                .short('C')
                .value_name("path")
                .action(clap::ArgAction::Set)
                .help(
                    "Run as if CKB was started in <path>, instead of the current working directory.",
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
        .subcommand(migrate());

    #[cfg(not(target_os = "windows"))]
    let command = command.subcommand(daemon());

    command
}

/// Parse the command line arguments by supplying the version information.
///
/// The version is used to generate the help message and output for `--version`.
pub fn get_bin_name_and_matches(version: &Version) -> (String, ArgMatches) {
    let bin_name = std::env::args()
        .next()
        .unwrap_or_else(|| BIN_NAME.to_owned());
    let matches = basic_app()
        .version(version.short())
        .long_version(version.long())
        .get_matches();
    (bin_name, matches)
}

fn run() -> Command {
    let command = Command::new(CMD_RUN)
        .about("Run CKB node")
        .arg(
            Arg::new(ARG_BA_ADVANCED)
                .long(ARG_BA_ADVANCED)
                .action(clap::ArgAction::SetTrue)
                .help("Allow any block assembler code hash and args"),
        )
        .arg(
            Arg::new(ARG_SKIP_CHAIN_SPEC_CHECK)
                .long(ARG_SKIP_CHAIN_SPEC_CHECK)
                .action(clap::ArgAction::SetTrue)
                .help("Skip checking the chain spec with the hash stored in the database"),
        ).arg(
        Arg::new(ARG_OVERWRITE_CHAIN_SPEC)
            .long(ARG_OVERWRITE_CHAIN_SPEC)
            .action(clap::ArgAction::SetTrue)
            .help("Overwrite the chain spec in the database with the present configured chain spec")
    ).arg(
        Arg::new(ARG_ASSUME_VALID_TARGET)
            .long(ARG_ASSUME_VALID_TARGET)
            .action(clap::ArgAction::Set)
            .value_parser(is_h256)
            .help(format!("This parameter specifies the hash of a block. \
When the height does not reach this block's height, script execution will be disabled, \
meaning it will skip the verification of the script content. \n\n\
Please note that when this option is enabled, the header will be synchronized to \
the highest block currently found. During this period, if the assume valid target is found, \
the block download starts; \
If the assume valid target is either absent or has a timestamp within 24 hours of the current time, \
the target considered invalid, and the block download proceeds with full verification. \n\n\n\
default(MainNet): {}\n
default(TestNet): {}\n\n
You can explicitly set the value to 0x0000000000000000000000000000000000000000000000000000000000000000 \
to disable the default behavior and execute full verification for all blocks, \
",
                          ckb_constant::latest_assume_valid_target::mainnet::DEFAULT_ASSUME_VALID_TARGET,
                          ckb_constant::latest_assume_valid_target::testnet::DEFAULT_ASSUME_VALID_TARGET))
    ).arg(
        Arg::new(ARG_INDEXER)
            .long(ARG_INDEXER)
            .action(clap::ArgAction::SetTrue)
            .help("Start the built-in indexer service"),
        )
        .arg(
            Arg::new(ARG_RICH_INDEXER)
            .long(ARG_RICH_INDEXER)
            .action(clap::ArgAction::SetTrue)
            .help("Start the built-in rich-indexer service"),
        );

    #[cfg(not(target_os = "windows"))]
    let command = command.arg(
        Arg::new(ARG_DAEMON)
            .long(ARG_DAEMON)
            .action(clap::ArgAction::SetTrue)
            .help(
                "Starts ckb as a daemon, \
                which will run in the background and output logs to the specified log file",
            ),
    );
    command
}

fn miner() -> Command {
    Command::new(CMD_MINER).about("Runs ckb miner").arg(
        Arg::new(ARG_LIMIT)
            .short('l')
            .long(ARG_LIMIT)
            .action(clap::ArgAction::Set)
            .value_parser(clap::value_parser!(u128))
            .default_value("0")
            .help(
                "Exit after finding this specific number of nonces; \
            0 means the miner will never exit. [default: 0]",
            ),
    )
}

fn reset_data() -> Command {
    Command::new(CMD_RESET_DATA)
        .about(
            "Truncate the database directory\n\
             Example:\n\
             ckb reset-data --force --database",
        )
        .arg(
            Arg::new(ARG_FORCE)
                .short('f')
                .long(ARG_FORCE)
                .action(clap::ArgAction::SetTrue)
                .help("Delete data without interactive prompt"),
        )
        .arg(
            Arg::new(ARG_ALL)
                .long(ARG_ALL)
                .action(clap::ArgAction::SetTrue)
                .help("Delete the whole data directory"),
        )
        .arg(
            Arg::new(ARG_DATABASE)
                .long(ARG_DATABASE)
                .action(clap::ArgAction::SetTrue)
                .help("Delete only `data/db`"),
        )
        .arg(
            Arg::new(ARG_INDEXER)
                .long(ARG_INDEXER)
                .action(clap::ArgAction::SetTrue)
                .help("Delete only `data/indexer/store`"),
        )
        .arg(
            Arg::new(ARG_RICH_INDEXER)
                .long(ARG_RICH_INDEXER)
                .action(clap::ArgAction::SetTrue)
                .help("Delete only `data/indexer/sqlite`"),
        )
        .arg(
            Arg::new(ARG_NETWORK)
                .long(ARG_NETWORK)
                .action(clap::ArgAction::SetTrue)
                .help("Delete both peer store and secret key"),
        )
        .arg(
            Arg::new(ARG_NETWORK_PEER_STORE)
                .long(ARG_NETWORK_PEER_STORE)
                .action(clap::ArgAction::SetTrue)
                .help("Delete only `data/network/peer_store`"),
        )
        .arg(
            Arg::new(ARG_NETWORK_SECRET_KEY)
                .long(ARG_NETWORK_SECRET_KEY)
                .action(clap::ArgAction::SetTrue)
                .help("Delete only `data/network/secret_key`"),
        )
        .arg(
            Arg::new(ARG_LOGS)
                .long(ARG_LOGS)
                .action(clap::ArgAction::SetTrue)
                .help("Delete only `data/logs`"),
        )
}

pub(crate) fn stats() -> Command {
    Command::new(CMD_STATS)
        .about(
            "Chain stats\n\
             Example:\n\
             ckb -C <dir> stats --from 1 --to 500",
        )
        .arg(
            Arg::new(ARG_FROM)
                .long(ARG_FROM)
                .value_parser(clap::value_parser!(u64))
                .action(clap::ArgAction::Set)
                .help("Specify from block number"),
        )
        .arg(
            Arg::new(ARG_TO)
                .long(ARG_TO)
                .value_parser(clap::value_parser!(u64))
                .action(clap::ArgAction::Set)
                .help("Specify to block number"),
        )
}

fn replay() -> Command {
    Command::new(CMD_REPLAY)
        .about("Replay CKB process block")
        .override_help("
            --tmp-target <tmp> --profile 1 10,\n
            --tmp-target <tmp> --sanity-check,\n
        ")
        .arg(Arg::new(ARG_TMP_TARGET).long(ARG_TMP_TARGET).value_parser(clap::builder::PathBufValueParser::new()).action(clap::ArgAction::Set).required(true).help(
            "Specify a target path. The profile command makes a temporary directory within the specified target path. This temporary directory will be automatically deleted when the command completes.",
        ))
        .arg(Arg::new(ARG_PROFILE).long(ARG_PROFILE).action(clap::ArgAction::SetTrue).help(
            "Enable profile",
        ))
        .arg(
            Arg::new(ARG_FROM)
                .value_parser(clap::value_parser!(u64))
                .help("Specify profile from block number"),
        )
        .arg(
            Arg::new(ARG_TO)
                .value_parser(clap::value_parser!(u64))
                .help("Specify profile to block number"),
        )
        .arg(
            Arg::new(ARG_SANITY_CHECK).long(ARG_SANITY_CHECK).action(clap::ArgAction::SetTrue).help("Enable sanity check")
        )
        .arg(
            Arg::new(ARG_FULL_VERIFICATION).long(ARG_FULL_VERIFICATION).action(clap::ArgAction::SetTrue).help("Enable sanity check")
        )
        .group(
            ArgGroup::new("mode")
                .args([ARG_PROFILE, ARG_SANITY_CHECK])
                .required(true)
        )
}

fn export() -> Command {
    Command::new(CMD_EXPORT).about("Export CKB data").arg(
        Arg::new(ARG_TARGET)
            .short('t')
            .long(ARG_TARGET)
            .value_name("path")
            .value_parser(clap::builder::PathBufValueParser::new())
            .required(true)
            .help("Specify the export target path"),
    )
}

fn import() -> Command {
    Command::new(CMD_IMPORT).about("Import CKB data").arg(
        Arg::new(ARG_SOURCE)
            .index(1)
            .value_name("path")
            .value_parser(clap::builder::PathBufValueParser::new())
            .required(true)
            .help("Specify the exported data path"),
    )
}

fn migrate() -> Command {
    Command::new(CMD_MIGRATE)
        .about("Run CKB migration")
        .arg(
            Arg::new(ARG_MIGRATE_CHECK)
                .long(ARG_MIGRATE_CHECK)
                .action(clap::ArgAction::SetTrue)
                .help(
                    "Perform database version check without migrating. \
                    If migration is in need, ExitCode(0) is returned; \
                    otherwise ExitCode(64) is returned",
                ),
        )
        .arg(
            Arg::new(ARG_FORCE)
                .long(ARG_FORCE)
                .action(clap::ArgAction::SetTrue)
                .conflicts_with(ARG_MIGRATE_CHECK)
                .help("Migrate without interactive prompt"),
        )
        .arg(
            Arg::new(ARG_INCLUDE_BACKGROUND)
                .long(ARG_INCLUDE_BACKGROUND)
                .action(clap::ArgAction::SetTrue)
                .help("Whether include background migrations"),
        )
}

#[cfg(not(target_os = "windows"))]
fn daemon() -> Command {
    Command::new(CMD_DAEMON)
        .about("Runs ckb daemon command")
        .arg(
            Arg::new(ARG_DAEMON_CHECK)
                .long(ARG_DAEMON_CHECK)
                .action(clap::ArgAction::SetTrue)
                .help("Check the daemon status"),
        )
        .arg(
            Arg::new(ARG_DAEMON_STOP)
                .long(ARG_DAEMON_STOP)
                .action(clap::ArgAction::SetTrue)
                .conflicts_with(ARG_DAEMON_CHECK)
                .help("Stop the daemon process, both the miner and the node"),
        )
}

fn list_hashes() -> Command {
    Command::new(CMD_LIST_HASHES)
        .about("List well known hashes")
        .arg(
            Arg::new(ARG_BUNDLED)
                .short('b')
                .long(ARG_BUNDLED)
                .action(clap::ArgAction::SetTrue)
                .help(
                    "List hashes of the bundled chain specs, instead of the current effective ones.",
                ),
        )
        .arg(
            Arg::new(ARG_FORMAT)
                .short('f')
                .long(ARG_FORMAT)
                .value_parser(["json", "toml"])
                .default_value("toml")
                .help("Set the format of the printed hashes"),
        )
}

fn init() -> Command {
    Command::new(CMD_INIT)
        .about("Create a CKB directory or re-initialize an existing one")
        .arg(
            Arg::new(ARG_INTERACTIVE)
                .short('i')
                .long(ARG_INTERACTIVE)
                .action(clap::ArgAction::SetTrue)
                .help("Interactive mode"),
        )
        .arg(
            Arg::new(ARG_LIST_CHAINS)
                .short('l')
                .long(ARG_LIST_CHAINS)
                .action(clap::ArgAction::SetTrue)
                .help("List available options for --chain"),
        )
        .arg(
            Arg::new(ARG_CHAIN)
                .short('c')
                .long(ARG_CHAIN)
                .value_parser(
                    AVAILABLE_SPECS
                        .iter()
                        .map(|v| v.to_string())
                        .collect::<Vec<_>>(),
                )
                .default_value(DEFAULT_SPEC)
                .help("Initialize CKB directory for <chain>"),
        )
        .arg(
            Arg::new(ARG_IMPORT_SPEC)
                .long(ARG_IMPORT_SPEC)
                .action(clap::ArgAction::Set)
                .help(
                    "Use the specified file as the chain spec. Specially, \
                     The dash \"-\" denotes importing the spec from stdin encoded in base64",
                ),
        )
        .arg(
            Arg::new(ARG_LOG_TO)
                .long(ARG_LOG_TO)
                .value_parser(["file", "stdout", "both"])
                .default_value("both")
                .help("Configure where the logs should be printed"),
        )
        .arg(
            Arg::new(ARG_FORCE)
                .short('f')
                .long(ARG_FORCE)
                .action(clap::ArgAction::SetTrue)
                .help("Enforce overwriting existing files"),
        )
        .arg(
            Arg::new(ARG_RPC_PORT)
                .long(ARG_RPC_PORT)
                .default_value(DEFAULT_RPC_PORT)
                .help("Replace CKB RPC port in the created config file"),
        )
        .arg(
            Arg::new(ARG_P2P_PORT)
                .long(ARG_P2P_PORT)
                .default_value(DEFAULT_P2P_PORT)
                .help("Replace CKB P2P port in the created config file"),
        )
        .arg(
            Arg::new(ARG_BA_CODE_HASH)
                .long(ARG_BA_CODE_HASH)
                .value_name("code_hash")
                .value_parser(is_h256)
                .action(clap::ArgAction::Set)
                .help(
                    "Set code_hash in [block_assembler] \
                     [default: secp256k1 if --ba-arg is present]",
                ),
        )
        .arg(
            Arg::new(ARG_BA_ARG)
                .long(ARG_BA_ARG)
                .value_name("arg")
                .action(clap::ArgAction::Append)
                .value_parser(is_hex)
                .help("Set args in [block_assembler]"),
        )
        .arg(
            Arg::new(ARG_BA_HASH_TYPE)
                .long(ARG_BA_HASH_TYPE)
                .value_name("hash_type")
                .action(clap::ArgAction::Set)
                .value_parser(["data", "type", "data1"])
                .default_value("type")
                .help("Set hash type in [block_assembler]"),
        )
        .group(
            ArgGroup::new(GROUP_BA)
                .args([ARG_BA_CODE_HASH, ARG_BA_ARG])
                .multiple(true),
        )
        .arg(
            Arg::new(ARG_BA_MESSAGE)
                .long(ARG_BA_MESSAGE)
                .value_name("message")
                .value_parser(is_hex)
                .requires(GROUP_BA)
                .help("Set message in [block_assembler]"),
        )
        .arg(Arg::new("export-specs").long("export-specs").hide(true))
        .arg(Arg::new("list-specs").long("list-specs").hide(true))
        .arg(
            Arg::new("spec")
                .short('s')
                .long("spec")
                .action(clap::ArgAction::Set)
                .hide(true),
        )
        .arg(
            Arg::new(ARG_GENESIS_MESSAGE)
                .long(ARG_GENESIS_MESSAGE)
                .value_name(ARG_GENESIS_MESSAGE)
                .action(clap::ArgAction::Set)
                .help(
                    "Specify a string as the genesis message. \
                     This only works for dev chains. \
                     If no message is provided, use the current timestamp.",
                ),
        )
}

fn peer_id() -> Command {
    Command::new(CMD_PEERID)
        .about("About peer id, base on Secp256k1")
        .subcommand_required(true)
        .subcommand(
            Command::new(CMD_FROM_SECRET)
                .about("Generate peer id from secret file")
                .arg(
                    Arg::new(ARG_SECRET_PATH)
                        .action(clap::ArgAction::Set)
                        .long(ARG_SECRET_PATH)
                        .required(true)
                        .help("Generate peer id from secret file path"),
                ),
        )
        .subcommand(
            Command::new(CMD_GEN_SECRET)
                .about("Generate random key to file")
                .arg(
                    Arg::new(ARG_SECRET_PATH)
                        .long(ARG_SECRET_PATH)
                        .required(true)
                        .action(clap::ArgAction::Set)
                        .help("Generate peer id to file path"),
                ),
        )
}

fn is_hex(hex: &str) -> Result<String, String> {
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
                    return Err(format!("Hex has invalid char: {invalid_char}"));
                }
            }
        }

        Ok(hex.to_string())
    } else {
        Err("Must 0x-prefixed hexadecimal string".to_string())
    }
}

fn is_h256(hex: &str) -> Result<String, String> {
    if hex.len() != 66 {
        Err("Must be 0x-prefixed hexadecimal string and string length is 66".to_owned())
    } else {
        is_hex(hex)
    }
}
