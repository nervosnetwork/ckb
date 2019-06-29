use ckb_build_info::Version;
use ckb_resource::{DEFAULT_P2P_PORT, DEFAULT_RPC_PORT, DEFAULT_SPEC};
use clap::{App, AppSettings, Arg, ArgGroup, ArgMatches, SubCommand};

pub const CMD_RUN: &str = "run";
pub const CMD_MINER: &str = "miner";
pub const CMD_EXPORT: &str = "export";
pub const CMD_IMPORT: &str = "import";
pub const CMD_INIT: &str = "init";
pub const CMD_PROF: &str = "prof";
pub const CMD_STATS: &str = "stats";
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
pub const ARG_BA_CODE_HASH: &str = "ba-code-hash";
pub const ARG_BA_ARG: &str = "ba-arg";
pub const ARG_BA_DATA: &str = "ba-data";
pub const ARG_FROM: &str = "from";
pub const ARG_TO: &str = "to";

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
        .subcommand(cli())
        .subcommand(init())
        .subcommand(prof())
        .subcommand(stats())
}

pub fn get_matches(version: &Version) -> ArgMatches<'static> {
    basic_app()
        .version(version.short().as_str())
        .long_version(version.long().as_str())
        .get_matches()
}

fn run() -> App<'static, 'static> {
    SubCommand::with_name(CMD_RUN).about("Runs ckb node")
}

fn miner() -> App<'static, 'static> {
    SubCommand::with_name(CMD_MINER).about("Runs ckb miner")
}

pub(crate) fn stats() -> App<'static, 'static> {
    SubCommand::with_name(CMD_STATS)
        .about(
            "Statics chain infomation\n\
             Example: \n\
             ckb -- -C <dir> stats -- from 1 --to 500",
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

fn prof() -> App<'static, 'static> {
    SubCommand::with_name(CMD_PROF)
        .about(
            "Profiles ckb node\n\
             Example: Process 1..500 blocks then output flagme graph\n\
             cargo flamegraph --bin ckb -- -C <dir> prof 1 500",
        )
        .arg(
            Arg::with_name(ARG_FROM)
                .required(true)
                .index(1)
                .help("Specifies from block number."),
        )
        .arg(
            Arg::with_name(ARG_TO)
                .required(true)
                .index(2)
                .help("Specifies to block number."),
        )
}

fn arg_format() -> Arg<'static, 'static> {
    Arg::with_name(ARG_FORMAT)
        .short("f")
        .long(ARG_FORMAT)
        .possible_values(&["bin", "json"])
        .required(true)
        .takes_value(true)
        .help("Specifies the format.")
}

fn export() -> App<'static, 'static> {
    SubCommand::with_name(CMD_EXPORT)
        .about("Exports ckb data")
        .arg(arg_format())
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
        .arg(arg_format())
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
                .possible_values(&["toml", "cmd"])
                .default_value("toml")
                .required(true)
                .takes_value(true)
                .help("Output format. toml: ckb.toml, cmd: command line options for `ckb init`"),
        )
}

fn init() -> App<'static, 'static> {
    SubCommand::with_name(CMD_INIT)
        .about("Creates a CKB direcotry or reinitializes an existing one")
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
        .group(ArgGroup::with_name(GROUP_BA).args(&[ARG_BA_CODE_HASH, ARG_BA_ARG]))
        .arg(
            Arg::with_name(ARG_BA_DATA)
                .long(ARG_BA_DATA)
                .value_name("data")
                .validator(is_hex)
                .requires(GROUP_BA)
                .help("Sets data in [block_assembler]"),
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

fn is_hex(hex: String) -> Result<(), String> {
    let tmp = hex.as_bytes();
    if tmp.len() < 2 {
        Err("Must be a 0x-prefixed hexadecimal string".to_string())
    } else if tmp.len() & 1 != 0 {
        Err("Hexadecimal strings must be of even length".to_string())
    } else if tmp[..2] == b"0x"[..] {
        for byte in &tmp[2..] {
            match byte {
                b'A'...b'F' | b'a'...b'f' | b'0'...b'9' => continue,
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
    fn ba_data_requires_ba_arg_or_ba_code_hash() {
        let ok_ba_arg = basic_app().get_matches_from_safe(&[
            "ckb",
            "init",
            "--ba-data",
            "0x00",
            "--ba-arg",
            "0x00",
        ]);
        let ok_ba_code_hash = basic_app().get_matches_from_safe(&[
            "ckb",
            "init",
            "--ba-data",
            "0x00",
            "--ba-code-hash",
            "0x00",
        ]);
        let err = basic_app().get_matches_from_safe(&["ckb", "init", "--ba-data", "0x00"]);

        assert!(
            ok_ba_arg.is_ok(),
            "--ba-data is ok with --ba-arg, but gets error: {:?}",
            ok_ba_arg.err()
        );
        assert!(
            ok_ba_code_hash.is_ok(),
            "--ba-data is ok with --ba-code-hash, but gets error: {:?}",
            ok_ba_code_hash.err()
        );
        assert!(
            err.is_err(),
            "--ba-data requires --ba-arg or --ba-code-hash"
        );

        let err = err.err().unwrap();
        assert_eq!(clap::ErrorKind::MissingRequiredArgument, err.kind);
        assert!(err
            .message
            .contains("The following required arguments were not provided"));
        assert!(err.message.contains("--ba-arg"));
        assert!(err.message.contains("--ba-code-hash"));
    }
}
