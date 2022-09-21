//! CKB command line arguments parser.
use ckb_build_info::Version;
use ckb_resource::{DEFAULT_P2P_PORT, DEFAULT_RPC_PORT, DEFAULT_SPEC};
use clap::{Arg, ArgGroup, ArgMatches, Command};

/// binary file name(ckb)
pub const BIN_NAME: &str = "ckb";

/// Subcommand `run`.
pub const CMD_RUN: &str = "run";
/// Subcommand `miner`.
pub const CMD_MINER: &str = "miner";
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
/// Subcommand `reset-data`.
pub const CMD_RESET_DATA: &str = "reset-data";
/// Subcommand `peer-id`.
pub const CMD_PEERID: &str = "peer-id";
/// Subcommand `gen`.
pub const CMD_GEN_SECRET: &str = "gen";
/// Subcommand `from-secret`.
pub const CMD_FROM_SECRET: &str = "from-secret";
/// Subcommand `migrate`.
pub const CMD_MIGRATE: &str = "migrate";
/// Subcommand `db-repair`.
pub const CMD_DB_REPAIR: &str = "db-repair";

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
/// Command line argument `--ba-advanced`.
pub const ARG_INDEXER: &str = "indexer";
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

/// Command line arguments group `ba` for block assembler.
const GROUP_BA: &str = "ba";

/// return root clap Command
pub fn basic_app<'help>() -> Command<'help> {
    Command::new(BIN_NAME)
        .author("Nervos Core Dev <dev@nervos.org>")
        .about("Nervos CKB - The Common Knowledge Base")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .arg(
            Arg::new(ARG_CONFIG_DIR)
                .global(true)
                .short('C')
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
        .subcommand(db_repair())
}

/// Parse the command line arguments by supplying the version information.
///
/// The version is used to generate the help message and output for `--version`.
pub fn get_bin_name_and_matches(version: &Version) -> (String, ArgMatches) {
    let bin_name = std::env::args()
        .next()
        .unwrap_or_else(|| BIN_NAME.to_owned());
    let matches = basic_app()
        .version(version.short().as_str())
        .long_version(version.long().as_str())
        .get_matches();
    (bin_name, matches)
}

fn run<'help>() -> Command<'help> {
    Command::new(CMD_RUN)
        .about("Runs ckb node")
        .arg(
            Arg::new(ARG_BA_ADVANCED)
                .long(ARG_BA_ADVANCED)
                .help("Allows any block assembler code hash and args"),
        )
        .arg(
            Arg::new(ARG_SKIP_CHAIN_SPEC_CHECK)
                .long(ARG_SKIP_CHAIN_SPEC_CHECK)
                .help("Skips checking the chain spec with the hash stored in the database"),
        ).arg(
            Arg::new(ARG_OVERWRITE_CHAIN_SPEC)
                .long(ARG_OVERWRITE_CHAIN_SPEC)
                .help("Overwrites the chain spec in the database with the present configured chain spec")
        ).arg(
        Arg::new(ARG_ASSUME_VALID_TARGET)
            .long(ARG_ASSUME_VALID_TARGET)
            .takes_value(true)
            .validator(is_h256)
            .help("This parameter specifies the hash of a block. \
            When the height does not reach this block's height, the execution of the script will be disabled, \
            that is, skip verifying the script content. \
            \
            It should be noted that when this option is enabled, the header is first synchronized to \
            the highest currently found. During this period, if the assume valid target is found, \
            the download of the block starts; If the assume valid target is not found or it's \
            timestamp within 24 hours of the current time, the target will automatically become invalid, \
            and the download of the block will be started with verify")
        ).arg(
            Arg::new(ARG_INDEXER)
            .long(ARG_INDEXER)
            .help("Start the built-in indexer service"),
        )
}

fn miner<'help>() -> Command<'help> {
    Command::new(CMD_MINER).about("Runs ckb miner").arg(
        Arg::new(ARG_LIMIT)
            .short('l')
            .long(ARG_LIMIT)
            .takes_value(true)
            .help(
                "Exit after how many nonces found; \
            0 means the miner will never exit. [default: 0]",
            ),
    )
}

fn reset_data<'help>() -> Command<'help> {
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
                .help("Delete data without interactive prompt"),
        )
        .arg(
            Arg::new(ARG_ALL)
                .long(ARG_ALL)
                .help("Delete the whole data directory"),
        )
        .arg(
            Arg::new(ARG_DATABASE)
                .long(ARG_DATABASE)
                .help("Delete only `data/db`"),
        )
        .arg(
            Arg::new(ARG_NETWORK)
                .long(ARG_NETWORK)
                .help("Delete both peer store and secret key"),
        )
        .arg(
            Arg::new(ARG_NETWORK_PEER_STORE)
                .long(ARG_NETWORK_PEER_STORE)
                .help("Delete only `data/network/peer_store`"),
        )
        .arg(
            Arg::new(ARG_NETWORK_SECRET_KEY)
                .long(ARG_NETWORK_SECRET_KEY)
                .help("Delete only `data/network/secret_key`"),
        )
        .arg(
            Arg::new(ARG_LOGS)
                .long(ARG_LOGS)
                .help("Delete only `data/logs`"),
        )
}

pub(crate) fn stats<'help>() -> Command<'help> {
    Command::new(CMD_STATS)
        .about(
            "Statics chain information\n\
             Example:\n\
             ckb -C <dir> stats --from 1 --to 500",
        )
        .arg(
            Arg::new(ARG_FROM)
                .long(ARG_FROM)
                .takes_value(true)
                .help("Specifies from block number."),
        )
        .arg(
            Arg::new(ARG_TO)
                .long(ARG_TO)
                .takes_value(true)
                .help("Specifies to block number."),
        )
}

fn replay<'help>() -> Command<'help> {
    Command::new(CMD_REPLAY)
        .about("replay ckb process block")
        .override_help("
            --tmp-target <tmp> --profile 1 10,\n
            --tmp-target <tmp> --sanity-check,\n
        ")
        .arg(Arg::new(ARG_TMP_TARGET).long(ARG_TMP_TARGET).takes_value(true).required(true).help(
            "Specifies a target path, prof command make a temporary directory inside of target and the directory will be automatically deleted when finished",
        ))
        .arg(Arg::new(ARG_PROFILE).long(ARG_PROFILE).help(
            "Enable profile",
        ))
        .arg(
            Arg::new(ARG_FROM)
              .help("Specifies profile from block number."),
        )
        .arg(
            Arg::new(ARG_TO)
              .help("Specifies profile to block number."),
        )
        .arg(
            Arg::new(ARG_SANITY_CHECK).long(ARG_SANITY_CHECK).help("Enable sanity check")
        )
        .arg(
            Arg::new(ARG_FULL_VERIFICATION).long(ARG_FULL_VERIFICATION).help("Enable sanity check")
        )
        .group(
            ArgGroup::new("mode")
                .args(&[ARG_PROFILE, ARG_SANITY_CHECK])
                .required(true)
        )
}

fn export<'help>() -> Command<'help> {
    Command::new(CMD_EXPORT).about("Exports ckb data").arg(
        Arg::new(ARG_TARGET)
            .short('t')
            .long(ARG_TARGET)
            .value_name("path")
            .required(true)
            .help("Specifies the export target path."),
    )
}

fn import<'help>() -> Command<'help> {
    Command::new(CMD_IMPORT).about("Imports ckb data").arg(
        Arg::new(ARG_SOURCE)
            .index(1)
            .value_name("path")
            .required(true)
            .help("Specifies the exported data path."),
    )
}

fn migrate<'help>() -> Command<'help> {
    Command::new(CMD_MIGRATE)
        .about("Runs ckb migration")
        .arg(Arg::new(ARG_MIGRATE_CHECK).long(ARG_MIGRATE_CHECK).help(
            "Perform database version check without migrating, \
                    if migration is in need ExitCode(0) is returned，\
                    otherwise ExitCode(64) is returned",
        ))
        .arg(
            Arg::new(ARG_FORCE)
                .long(ARG_FORCE)
                .conflicts_with(ARG_MIGRATE_CHECK)
                .help("Do migration without interactive prompt"),
        )
}

fn db_repair<'help>() -> Command<'help> {
    Command::new(CMD_DB_REPAIR).about("Try repair ckb database")
}

fn list_hashes<'help>() -> Command<'help> {
    Command::new(CMD_LIST_HASHES)
        .about("Lists well known hashes")
        .arg(
            Arg::new(ARG_BUNDLED).short('b').long(ARG_BUNDLED).help(
                "Lists hashes of the bundled chain specs instead of the current effective one.",
            ),
        )
        .arg(
            Arg::new(ARG_FORMAT)
                .short('f')
                .long(ARG_FORMAT)
                .possible_values(&["json", "toml"])
                .default_value("toml")
                .help("Set the format of the printed hashes."),
        )
}

fn init<'help>() -> Command<'help> {
    Command::new(CMD_INIT)
        .about("Creates a CKB directory or re-initializes an existing one")
        .arg(
            Arg::new(ARG_INTERACTIVE)
                .short('i')
                .long(ARG_INTERACTIVE)
                .help("Interactive mode"),
        )
        .arg(
            Arg::new(ARG_LIST_CHAINS)
                .short('l')
                .long(ARG_LIST_CHAINS)
                .help("Lists available options for --chain"),
        )
        .arg(
            Arg::new(ARG_CHAIN)
                .short('c')
                .long(ARG_CHAIN)
                .default_value(DEFAULT_SPEC)
                .help("Initializes CKB directory for <chain>"),
        )
        .arg(
            Arg::new(ARG_IMPORT_SPEC)
                .long(ARG_IMPORT_SPEC)
                .takes_value(true)
                .help(
                    "Uses the specifies file as chain spec. Specially, \
                     The dash \"-\" denotes importing the spec from stdin encoded in base64",
                ),
        )
        .arg(
            Arg::new(ARG_LOG_TO)
                .long(ARG_LOG_TO)
                .possible_values(&["file", "stdout", "both"])
                .default_value("both")
                .help("Configures where the logs should print"),
        )
        .arg(
            Arg::new(ARG_FORCE)
                .short('f')
                .long(ARG_FORCE)
                .help("Forces overwriting existing files"),
        )
        .arg(
            Arg::new(ARG_RPC_PORT)
                .long(ARG_RPC_PORT)
                .default_value(DEFAULT_RPC_PORT)
                .help("Replaces CKB RPC port in the created config file"),
        )
        .arg(
            Arg::new(ARG_P2P_PORT)
                .long(ARG_P2P_PORT)
                .default_value(DEFAULT_P2P_PORT)
                .help("Replaces CKB P2P port in the created config file"),
        )
        .arg(
            Arg::new(ARG_BA_CODE_HASH)
                .long(ARG_BA_CODE_HASH)
                .value_name("code_hash")
                .validator(is_h256)
                .takes_value(true)
                .help(
                    "Sets code_hash in [block_assembler] \
                     [default: secp256k1 if --ba-arg is present]",
                ),
        )
        .arg(
            Arg::new(ARG_BA_ARG)
                .long(ARG_BA_ARG)
                .value_name("arg")
                .validator(is_hex)
                .multiple_occurrences(true)
                .number_of_values(1)
                .help("Sets args in [block_assembler]"),
        )
        .arg(
            Arg::new(ARG_BA_HASH_TYPE)
                .long(ARG_BA_HASH_TYPE)
                .value_name("hash_type")
                .takes_value(true)
                .possible_values(&["data", "type", "data1"])
                .default_value("type")
                .help("Sets hash type in [block_assembler]"),
        )
        .group(
            ArgGroup::new(GROUP_BA)
                .args(&[ARG_BA_CODE_HASH, ARG_BA_ARG])
                .multiple(true),
        )
        .arg(
            Arg::new(ARG_BA_MESSAGE)
                .long(ARG_BA_MESSAGE)
                .value_name("message")
                .validator(is_hex)
                .requires(GROUP_BA)
                .help("Sets message in [block_assembler]"),
        )
        .arg(Arg::new("export-specs").long("export-specs").hide(true))
        .arg(Arg::new("list-specs").long("list-specs").hide(true))
        .arg(
            Arg::new("spec")
                .short('s')
                .long("spec")
                .takes_value(true)
                .hide(true),
        )
        .arg(
            Arg::new(ARG_GENESIS_MESSAGE)
                .long(ARG_GENESIS_MESSAGE)
                .value_name(ARG_GENESIS_MESSAGE)
                .takes_value(true)
                .help(
                    "Specify a string as the genesis message. \
                     Only works for dev chains. \
                     If no message is provided, use current timestamp.",
                ),
        )
}

fn peer_id<'help>() -> Command<'help> {
    Command::new(CMD_PEERID)
        .about("About peer id, base on Secp256k1")
        .subcommand_required(true)
        .subcommand(
            Command::new(CMD_FROM_SECRET)
                .about("Generate peer id from secret file")
                .arg(
                    Arg::new(ARG_SECRET_PATH)
                        .takes_value(true)
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
                        .takes_value(true)
                        .help("Generate peer id to file path"),
                ),
        )
}

fn is_hex(hex: &str) -> Result<(), String> {
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

fn is_h256(hex: &str) -> Result<(), String> {
    if hex.len() != 66 {
        Err("Must be 0x-prefixed hexadecimal string and string length is 66".to_owned())
    } else {
        is_hex(hex)
    }
}
