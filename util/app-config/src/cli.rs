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
pub const CMD_SECP256K1: &str = "secp256k1";
pub const CMD_HASHES: &str = "hashes";

pub const ARG_CONFIG_DIR: &str = "config-dir";
pub const ARG_FORMAT: &str = "format";
pub const ARG_TARGET: &str = "target";
pub const ARG_SOURCE: &str = "source";
pub const ARG_LIST_SPECS: &str = "list-specs";
pub const ARG_SPEC: &str = "spec";
pub const ARG_EXPORT_SPECS: &str = "export-specs";
pub const ARG_P2P_PORT: &str = "p2p-port";
pub const ARG_RPC_PORT: &str = "rpc-port";
pub const ARG_FORCE: &str = "force";
pub const ARG_LOG_TO: &str = "log-to";
pub const ARG_BUNDLED: &str = "bundled";
pub const ARG_GENERATE: &str = "generate";
pub const ARG_PRIVKEY: &str = "privkey";
pub const ARG_PUBKEY: &str = "pubkey";

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
        .subcommand(cli_secp256k1())
        .subcommand(cli_hashes())
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

fn cli_secp256k1() -> App<'static, 'static> {
    SubCommand::with_name(CMD_SECP256K1)
        .about("Use secp256k1 in [block_assember]")
        .arg(
            Arg::with_name(ARG_GENERATE)
                .long(ARG_GENERATE)
                .short("g")
                .requires(ARG_PRIVKEY)
                .help(
                    "Generate the privkey and save it into the file. \
                     Then print [block_assember] from the privkey",
                ),
        )
        .arg(
            Arg::with_name(ARG_PRIVKEY)
                .long(ARG_PRIVKEY)
                .value_name("path")
                .takes_value(true)
                .help(
                    "Read privkey from the file, or write generated privkey into the file \
                     when `--generate` is specified.",
                ),
        )
        .arg(
            Arg::with_name(ARG_PUBKEY)
                .long(ARG_PUBKEY)
                .value_name("path")
                .takes_value(true)
                .required_unless(ARG_PRIVKEY)
                .help(
                    "Read pubkey from the file, or write generated pubkey into the file \
                     when `--generate` is specified",
                ),
        )
}

fn init() -> App<'static, 'static> {
    SubCommand::with_name(CMD_INIT)
        .about("Create a CKB direcotry or reinitialize an existing one")
        .arg(
            Arg::with_name(ARG_LIST_SPECS)
                .short("l")
                .long(ARG_LIST_SPECS)
                .help("List available chain specs"),
        )
        .arg(
            Arg::with_name(ARG_SPEC)
                .short("s")
                .long(ARG_SPEC)
                .default_value(DEFAULT_SPEC)
                .help("Export config files for <spec>"),
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
                .help("Replace CKB RPC port in the exported config file"),
        )
        .arg(
            Arg::with_name(ARG_P2P_PORT)
                .long(ARG_P2P_PORT)
                .default_value(DEFAULT_P2P_PORT)
                .help("Replace CKB P2P port in the exported config file"),
        )
        .arg(
            Arg::with_name(ARG_EXPORT_SPECS)
                .long(ARG_EXPORT_SPECS)
                .hidden(true)
                .help("Export spec files as well"),
        )
}
