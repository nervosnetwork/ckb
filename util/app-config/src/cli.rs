use build_info::Version;
use ckb_resource::{DEFAULT_P2P_PORT, DEFAULT_RPC_PORT, DEFAULT_SPEC};
use clap::{App, AppSettings, Arg, ArgMatches, SubCommand};

pub const CMD_RUN: &str = "run";
pub const CMD_MINER: &str = "miner";
pub const CMD_EXPORT: &str = "export";
pub const CMD_IMPORT: &str = "import";
pub const CMD_INIT: &str = "init";
pub const CMD_PROF: &str = "prof";
pub const CMD_CLI: &str = "cli";
pub const CMD_HASHES: &str = "hashes";
pub const CMD_BLAKE256: &str = "blake256";
pub const CMD_BLAKE160: &str = "blake160";
pub const CMD_SECP256K1_LOCK: &str = "secp256k1-lock";

pub const ARG_CONFIG_DIR: &str = "config-dir";
pub const ARG_FORMAT: &str = "format";
pub const ARG_TARGET: &str = "target";
pub const ARG_SOURCE: &str = "source";
pub const ARG_DATA: &str = "data";
pub const ARG_LIST_CHAINS: &str = "list-chains";
pub const ARG_CHAIN: &str = "chain";
pub const ARG_P2P_PORT: &str = "p2p-port";
pub const ARG_RPC_PORT: &str = "rpc-port";
pub const ARG_FORCE: &str = "force";
pub const ARG_LOG_TO: &str = "log-to";
pub const ARG_BUNDLED: &str = "bundled";

pub fn get_matches(version: &Version) -> ArgMatches<'static> {
    App::new("ckb")
        .author("Nervos Core Dev <dev@nervos.org>")
        .about("Nervos CKB - The Common Knowledge Base")
        .version(version.short().as_str())
        .long_version(version.long().as_str())
        .setting(AppSettings::SubcommandRequiredElseHelp)
        .arg(
            Arg::with_name(ARG_CONFIG_DIR)
                .global(true)
                .short("C")
                .value_name("path")
                .takes_value(true)
                .help(
                    "Run as if ckb was started in <path> instead of the current working directory.",
                ),
        )
        .subcommand(run())
        .subcommand(miner())
        .subcommand(export())
        .subcommand(import())
        .subcommand(cli())
        .subcommand(init())
        .subcommand(prof())
        .get_matches()
}

fn run() -> App<'static, 'static> {
    SubCommand::with_name(CMD_RUN).about("Running ckb node")
}

fn miner() -> App<'static, 'static> {
    SubCommand::with_name(CMD_MINER).about("Running ckb miner")
}

fn prof() -> App<'static, 'static> {
    SubCommand::with_name(CMD_PROF)
        .about(
            "Profling ckb node\n\
             Example: Process 1..500 blocks then output flagme graph\n\
             cargo flamegraph --bin ckb -- -C <dir> prof 1 500",
        )
        .arg(
            Arg::with_name("from")
                .required(true)
                .index(1)
                .help("from block number."),
        )
        .arg(
            Arg::with_name("to")
                .required(true)
                .index(2)
                .help("to block number."),
        )
}

fn arg_format() -> Arg<'static, 'static> {
    Arg::with_name(ARG_FORMAT)
        .short("f")
        .long(ARG_FORMAT)
        .possible_values(&["bin", "json"])
        .required(true)
        .takes_value(true)
        .help("Specify the format.")
}

fn export() -> App<'static, 'static> {
    SubCommand::with_name(CMD_EXPORT)
        .about("Export ckb data")
        .arg(arg_format())
        .arg(
            Arg::with_name(ARG_TARGET)
                .short("t")
                .long(ARG_TARGET)
                .value_name("path")
                .required(true)
                .index(1)
                .help("Specify the export target path."),
        )
}

fn import() -> App<'static, 'static> {
    SubCommand::with_name(CMD_IMPORT)
        .about("Import ckb data")
        .arg(arg_format())
        .arg(
            Arg::with_name(ARG_SOURCE)
                .short("s")
                .long(ARG_SOURCE)
                .value_name("path")
                .required(true)
                .index(1)
                .help("Specify the exported data path."),
        )
}

fn cli() -> App<'static, 'static> {
    SubCommand::with_name(CMD_CLI)
        .about("CLI tools")
        .setting(AppSettings::SubcommandRequiredElseHelp)
        .subcommand(cli_hashes())
        .subcommand(cli_blake256())
        .subcommand(cli_blake160())
        .subcommand(cli_secp256k1_lock())
}

fn cli_hashes() -> App<'static, 'static> {
    SubCommand::with_name(CMD_HASHES)
        .about("List well known hashes")
        .arg(
            Arg::with_name(ARG_BUNDLED)
                .short("b")
                .long(ARG_BUNDLED)
                .help(
                    "List hashes of the bundled chain specs instead of the current effective one.",
                ),
        )
}

fn arg_hex_data() -> Arg<'static, 'static> {
    Arg::with_name(ARG_DATA)
        .short("d")
        .long(ARG_DATA)
        .value_name("hex")
        .required(true)
        .index(1)
        .help("The data encoded in hex.")
}

fn cli_blake256() -> App<'static, 'static> {
    SubCommand::with_name(CMD_BLAKE256)
        .about("Hashes data using blake2b with CKB personal option, prints first 256 bits.")
        .arg(arg_hex_data())
}

fn cli_blake160() -> App<'static, 'static> {
    SubCommand::with_name(CMD_BLAKE160)
        .about("Hashes data using blake2b with CKB personal option, prints first 160 bits.")
        .arg(arg_hex_data())
}

fn cli_secp256k1_lock() -> App<'static, 'static> {
    SubCommand::with_name(CMD_SECP256K1_LOCK)
        .about("Prints lock structure from secp256k1 pubkey")
        .arg(
            Arg::with_name(ARG_DATA)
                .short("d")
                .long(ARG_DATA)
                .required(true)
                .index(1)
                .help("Pubkey encoded in hex, either uncompressed 65 bytes or compresed 33 bytes"),
        )
        .arg(
            Arg::with_name(ARG_FORMAT)
                .long(ARG_FORMAT)
                .short("s")
                .possible_values(&["block_assembler", "json"])
                .default_value("block_assembler")
                .required(true)
                .takes_value(true)
                .help("Output format: `block_assembler` is used in ckb.toml."),
        )
}

fn init() -> App<'static, 'static> {
    SubCommand::with_name(CMD_INIT)
        .about("Create a CKB direcotry or reinitialize an existing one")
        .arg(
            Arg::with_name(ARG_LIST_CHAINS)
                .short("l")
                .long(ARG_LIST_CHAINS)
                .help("List available options for --chain"),
        )
        .arg(
            Arg::with_name(ARG_CHAIN)
                .short("c")
                .long(ARG_CHAIN)
                .default_value(DEFAULT_SPEC)
                .help("Init CKB direcotry for <chain>"),
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
                .help("Force overwriting existing files"),
        )
        .arg(
            Arg::with_name(ARG_RPC_PORT)
                .long(ARG_RPC_PORT)
                .default_value(DEFAULT_RPC_PORT)
                .help("Set CKB RPC port"),
        )
        .arg(
            Arg::with_name(ARG_P2P_PORT)
                .long(ARG_P2P_PORT)
                .default_value(DEFAULT_P2P_PORT)
                .help("Set CKB P2P port"),
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
