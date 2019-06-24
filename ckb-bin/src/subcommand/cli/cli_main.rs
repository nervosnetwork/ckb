use std::collections::HashMap;
use std::env;
use std::fs;
use std::io::{self, Read};
use std::iter::FromIterator;
use std::process;
use std::sync::Arc;

use ckb_build_info::Version;
use ckb_sdk::HttpRpcClient;
use ckb_util::RwLock;
use clap::{crate_version, App, AppSettings, Arg, ArgMatches, SubCommand};

#[cfg(unix)]
use super::subcommands::TuiSubCommand;

use super::interactive::InteractiveEnv;
use super::subcommands::{
    start_index_thread, AccountSubCommand, CliSubCommand, IndexThreadState, LocalSubCommand,
    RpcSubCommand, WalletSubCommand,
};
use crate::cli_cmds::{
    ARG_BUNDLED, ARG_DATA, ARG_FORMAT, CMD_BLAKE160, CMD_BLAKE256, CMD_HASHES, CMD_SECP256K1_LOCK,
};
use crate::utils::{
    arg_parser::{ArgParser, UrlParser},
    config::GlobalConfig,
    other::get_key_store,
    printer::{ColorWhen, OutputFormat},
};

pub fn cli_main(version: Version, matches: &ArgMatches) -> Result<(), io::Error> {
    env_logger::init();

    #[cfg(not(unix))]
    let _enabled = ansi_term::enable_ansi_support();

    let mut env_map: HashMap<String, String> = HashMap::from_iter(env::vars());
    let api_uri_opt = matches
        .value_of("url")
        .map(ToOwned::to_owned)
        .or_else(|| env_map.remove("API_URL"));

    let mut ckb_cli_dir = dirs::home_dir().unwrap();
    ckb_cli_dir.push(".ckb-cli");
    let mut resource_dir = ckb_cli_dir.clone();
    resource_dir.push("resource");
    let mut index_dir = ckb_cli_dir.clone();
    index_dir.push("index");
    let index_state = Arc::new(RwLock::new(IndexThreadState::default()));

    let mut config = GlobalConfig::new(version, api_uri_opt.clone(), Arc::clone(&index_state));
    let mut config_file = ckb_cli_dir.clone();
    config_file.push("config");

    let mut output_format = OutputFormat::Yaml;
    if config_file.as_path().exists() {
        let mut file = fs::File::open(&config_file)?;
        let mut content = String::new();
        file.read_to_string(&mut content)?;
        let configs: serde_json::Value = serde_json::from_str(content.as_str()).unwrap();
        if api_uri_opt.is_none() {
            if let Some(value) = configs["url"].as_str() {
                config.set_url(value.to_string());
            }
        }
        config.set_debug(configs["debug"].as_bool().unwrap_or(false));
        config.set_color(configs["color"].as_bool().unwrap_or(true));
        output_format =
            OutputFormat::from_str(&configs["output_format"].as_str().unwrap_or("yaml"))
                .unwrap_or(OutputFormat::Yaml);
        config.set_output_format(output_format);
        config.set_completion_style(configs["completion_style"].as_bool().unwrap_or(true));
        config.set_edit_style(configs["edit_style"].as_bool().unwrap_or(true));
    }

    let api_uri = config.get_url().to_string();
    let index_controller = start_index_thread(api_uri.as_str(), index_dir.clone(), index_state);
    let mut rpc_client = HttpRpcClient::from_uri(api_uri.as_str());

    let color = ColorWhen::new(!matches.is_present("no-color")).color();
    if let Some(format) = matches.value_of("output-format") {
        output_format = OutputFormat::from_str(format).unwrap();
    }
    let result = match matches.subcommand() {
        #[cfg(unix)]
        ("tui", _) => TuiSubCommand::new(
            api_uri.to_string(),
            index_dir.clone(),
            index_controller.clone(),
        )
        .start(),
        ("rpc", Some(sub_matches)) => {
            RpcSubCommand::new(&mut rpc_client).process(&sub_matches, output_format, color)
        }
        ("local", Some(sub_matches)) => get_key_store(&ckb_cli_dir).and_then(|mut key_store| {
            LocalSubCommand::new(&mut rpc_client, &mut key_store, None, resource_dir.clone())
                .process(&sub_matches, output_format, color)
        }),
        ("account", Some(sub_matches)) => get_key_store(&ckb_cli_dir).and_then(|mut key_store| {
            AccountSubCommand::new(&mut key_store).process(&sub_matches, output_format, color)
        }),
        ("wallet", Some(sub_matches)) => get_key_store(&ckb_cli_dir).and_then(|mut key_store| {
            WalletSubCommand::new(
                &mut rpc_client,
                &mut key_store,
                None,
                index_dir.clone(),
                index_controller.clone(),
                false,
            )
            .process(&sub_matches, output_format, color)
        }),
        _ => {
            if let Err(err) =
                InteractiveEnv::from_config(ckb_cli_dir, config, index_controller.clone())
                    .and_then(|mut env| env.start())
            {
                eprintln!("Process error: {}", err);
                index_controller.shutdown();
                process::exit(1);
            }
            index_controller.shutdown();
            process::exit(0)
        }
    };

    match result {
        Ok(message) => {
            println!("{}", message);
            index_controller.shutdown();
        }
        Err(err) => {
            eprintln!("{}", err);
            index_controller.shutdown();
            process::exit(1);
        }
    }
    Ok(())
}

pub fn build_cli() -> App<'static, 'static> {
    let app = SubCommand::with_name("cli")
        .about("CLI tools")
        .subcommand(cli_hashes())
        .subcommand(cli_blake256())
        .subcommand(cli_blake160())
        .subcommand(cli_secp256k1_lock())
        .subcommand(RpcSubCommand::subcommand())
        .subcommand(AccountSubCommand::subcommand("account"))
        .subcommand(WalletSubCommand::subcommand())
        .arg(
            Arg::with_name("url")
                .long("url")
                .takes_value(true)
                .validator(|input| UrlParser.validate(input))
                .help("RPC API server url"),
        )
        .arg(
            Arg::with_name("output-format")
                .long("output-format")
                .takes_value(true)
                .possible_values(&["yaml", "json"])
                .default_value("yaml")
                .global(true)
                .help("Select output format"),
        )
        .arg(
            Arg::with_name("no-color")
                .long("no-color")
                .global(true)
                .help("Do not highlight(color) output json"),
        )
        .arg(
            Arg::with_name("debug")
                .long("debug")
                .global(true)
                .help("Display request parameters"),
        );

    let app = app.subcommand(LocalSubCommand::subcommand());

    #[cfg(unix)]
    let app = app.subcommand(SubCommand::with_name("tui").about("Enter TUI mode"));

    app
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

pub fn build_interactive() -> App<'static, 'static> {
    App::new("interactive")
        .version(crate_version!())
        .global_setting(AppSettings::NoBinaryName)
        .global_setting(AppSettings::ColoredHelp)
        .global_setting(AppSettings::DeriveDisplayOrder)
        .global_setting(AppSettings::DisableVersion)
        .subcommand(
            SubCommand::with_name("config")
                .about("Config environment")
                .arg(
                    Arg::with_name("url")
                        .long("url")
                        .validator(|input| UrlParser.validate(input))
                        .takes_value(true)
                        .help("Config RPC API url"),
                )
                .arg(
                    Arg::with_name("color")
                        .long("color")
                        .help("Switch color for rpc interface"),
                )
                .arg(
                    Arg::with_name("debug")
                        .long("debug")
                        .help("Switch debug mode"),
                )
                .arg(
                    Arg::with_name("output-format")
                        .long("output-format")
                        .takes_value(true)
                        .possible_values(&["yaml", "json"])
                        .default_value("yaml")
                        .help("Select output format"),
                )
                .arg(
                    Arg::with_name("completion_style")
                        .long("completion_style")
                        .help("Switch completion style"),
                )
                .arg(
                    Arg::with_name("edit_style")
                        .long("edit_style")
                        .help("Switch edit style"),
                ),
        )
        .subcommand(SubCommand::with_name("info").about("Display global variables"))
        .subcommand(
            SubCommand::with_name("exit")
                .visible_alias("quit")
                .about("Exit the interactive interface"),
        )
        .subcommand(RpcSubCommand::subcommand())
        .subcommand(AccountSubCommand::subcommand("account"))
        .subcommand(WalletSubCommand::subcommand())
        .subcommand(LocalSubCommand::subcommand())
}
