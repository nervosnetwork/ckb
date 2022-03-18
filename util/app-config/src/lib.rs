//! CKB command line arguments and config options.
use std::{path::PathBuf, str::FromStr};

use clap::{Args, Parser, Subcommand};

pub use app_config::{
    AppConfig, CKBAppConfig, ChainConfig, LogConfig, MetricsConfig, MinerAppConfig,
};
pub use args::{
    ExportArgs, ImportArgs, InitArgs, MigrateArgs, MinerArgs, PeerIDArgs, RepairArgs, ReplayArgs,
    ResetDataArgs, RunArgs, StatsArgs,
};
use ckb_chain_spec::{consensus::Consensus, ChainSpec};
use ckb_jsonrpc_types::ScriptHashType;
use ckb_types::{u256, H256, U256};
pub use configs::*;
pub use exit_code::ExitCode;
#[cfg(feature = "with_sentry")]
pub use sentry_config::SentryConfig;

mod app_config;
mod args;
pub mod cli;
mod configs;
mod exit_code;
pub(crate) mod legacy;
#[cfg(feature = "with_sentry")]
mod sentry_config;

#[cfg(test)]
mod tests;

const CMD_RUN: &str = "run";
const CMD_MINER: &str = "miner";
const CMD_EXPORT: &str = "export";
const CMD_IMPORT: &str = "import";
const CMD_INIT: &str = "init";
const CMD_REPLAY: &str = "replay";
const CMD_STATS: &str = "stats";
const CMD_LIST_HASHES: &str = "list-hashes";
const CMD_RESET_DATA: &str = "reset-data";
const CMD_PEERID: &str = "peer-id";
const CMD_MIGRATE: &str = "migrate";
const CMD_DB_REPAIR: &str = "db-repair";

// 500_000 total difficulty
const MIN_CHAIN_WORK_500K: U256 = u256!("0x3314412053c82802a7");

#[derive(Parser)]
#[clap(
    version,
    author = "Nervos Core Dev <dev@nervos.org>",
    about = "Nervos CKB - The Common Knowledge Base"
)]
/// ckb command line structure for clap parsed
pub struct CkbCli {
    #[clap(short = 'C', parse(from_os_str), value_name = "Path", global = true)]
    /// Runs as if ckb was started in <Path> instead of the current working directory
    pub config: Option<PathBuf>,

    /// ckb subcommand
    #[clap(subcommand)]
    pub sub_command: CKBSubCommand,
}

#[derive(Subcommand)]
#[clap()]
/// ckb subcommand
pub enum CKBSubCommand {
    ///run subcommand
    #[clap(about = "Runs ckb node", long_about = None)]
    Run(CmdRun),
    /// miner subcommand
    #[clap(about = "Runs ckb miner")]
    Miner(CmdMiner),
    /// export subcommand
    #[clap(about = "Exports ckb data")]
    Export(CmdExport),
    /// import subcommand
    #[clap(about = "Imports ckb data")]
    Import(CmdImport),
    /// init subcommand
    #[clap(about = "Creates a CKB directory or re-initializes an existing one")]
    Init(CmdInit),
    /// replay subcommand
    #[clap(about = "Replay ckb process block")]
    Replay(CmdReplay),
    /// Stats subcommand
    #[clap(about = "Statics chain information")]
    Stats(CmdStats),
    /// list-hashes subcommand
    #[clap(about = "Lists well known hashes")]
    ListHashes(CmdListHashes),
    /// reset-data subcommand
    #[clap(about = "Truncate the database directory")]
    ResetData(CmdResetData),
    /// peerid subcommand
    #[clap(subcommand)]
    PeerId(PeeridSubCommand),
    /// migrate subcommand
    #[clap(about = "Runs ckb migration")]
    Migrate(CmdMigrate),
    /// db-repair subcommand
    #[clap(about = "Try repair ckb database")]
    DbRepair(CmdDbRepair),
}
impl ToString for CKBSubCommand {
    fn to_string(&self) -> String {
        match self {
            CKBSubCommand::Run(_) => CMD_RUN.to_owned(),
            CKBSubCommand::Miner(_) => CMD_MINER.to_owned(),
            CKBSubCommand::Export(_) => CMD_EXPORT.to_owned(),
            CKBSubCommand::Import(_) => CMD_IMPORT.to_owned(),
            CKBSubCommand::Init(_) => CMD_INIT.to_owned(),
            CKBSubCommand::Replay(_) => CMD_REPLAY.to_owned(),
            CKBSubCommand::Stats(_) => CMD_STATS.to_owned(),
            CKBSubCommand::ListHashes(_) => CMD_LIST_HASHES.to_owned(),
            CKBSubCommand::ResetData(_) => CMD_RESET_DATA.to_owned(),
            CKBSubCommand::PeerId(_) => CMD_PEERID.to_owned(),
            CKBSubCommand::Migrate(_) => CMD_MIGRATE.to_owned(),
            CKBSubCommand::DbRepair(_) => CMD_DB_REPAIR.to_owned(),
        }
    }
}

/// Run subcommand
#[derive(Args)]
#[clap()]
pub struct CmdRun {
    #[clap(short, long, validator=is_hash256)]
    /// This parameter specifies the hash of a block.
    /// When the height does not reach this block's height, the execution of the script will be disabled,
    /// that is, skip verifying the script content.
    /// It should be noted that when this option is enabled, the header is first synchronized to
    /// the highest currently found. During this period, if the assume valid target is found,
    /// the download of the block starts; If the assume valid target is not found or it's
    /// timestamp within 24 hours of the current time, the target will automatically become invalid,
    /// and the download of the block will be started with verify
    assume_valid_target: Option<String>,

    #[clap(short, long)]
    ///Allows any block assembler code hash and args
    ba_advanced: bool,

    #[clap(short, long)]
    /// Skips checking the chain spec with the hash stored in the database
    skip_spec_check: bool,

    #[clap(short, long)]
    /// Overwrites the chain spec in the database with the present configured chain spec
    overwrite_spec: bool,
}

/// miner subcommand
#[derive(Args)]
#[clap()]
pub struct CmdMiner {
    #[clap(
        short,
        long,
        help = "Exit after how many nonces found; 0 means the miner will never exit. [default: 0]"
    )]
    /// The miner process will exit when there are `limit` nonces (puzzle solutions) found. Set it
    /// to 0 to loop forever.
    limit: u128,
}

/// export subcommand
#[derive(Args)]
#[clap()]
pub struct CmdExport {
    #[clap(short, long, value_name = "Path")]
    /// Specifies the export data path
    target: PathBuf,
}

/// import subcommand
#[derive(Args)]
#[clap()]
pub struct CmdImport {
    #[clap(short, long, value_name = "Path")]
    /// Specifies the import data path
    source: PathBuf,
}

/// init subcommand
#[derive(Args)]
#[clap()]
pub struct CmdInit {
    #[clap(from_global)]
    config: Option<PathBuf>,

    #[clap(long, long_help = "Sets args in [block_assembler]")]
    /// Block assembler lock script args
    ba_arg: Option<Vec<String>>,

    #[clap(
        long,
        long_help = "Sets code_hash in [block_assembler] [default: secp256k1 if --ba-arg is present]"
    )]
    /// Block assembler lock script code hash
    ba_code_hash: Option<String>,

    #[clap(
        long,
        long_help = "Sets hash type in [block_assembler] [default: type] [possible values: data, type, data1]",
        default_value = "type", possible_values = ["data", "type", "data1"]
    )]
    /// Block assembler lock script hash type
    ba_hash_type: String,

    #[clap(long, long_help = "Sets message in [block_assembler]")]
    /// Block assembler cellbase transaction message
    ba_message: Option<String>,

    #[clap(
        short,
        long,
        long_help = "Initializes CKB directory for <chain> [default: mainnet]",
        default_value = "mainnet"
    )]
    /// The chain name that this node will join
    chain: String,

    #[clap(short, long)]
    /// Force file overwriting
    pub force: bool,

    #[clap(short, long)]
    /// Specify a string as the genesis message. Only works for dev chains. If no message is
    /// provided, use current timestamp
    genesis_message: Option<String>,

    #[clap(short, long)]
    /// Whether to prompt user inputs interactively.
    interactive: bool,

    #[clap(short, long)]
    /// Import the spec file.
    ///
    /// When this is set to `-`, the spec file is imported from stdin and the file content must be
    /// encoded by base64. Otherwise it must be a path to the spec file.
    ///
    /// The spec file will be saved into `specs/{CHAIN}.toml`, where `CHAIN` is the chain name.
    import_spec: Option<String>,

    #[clap(short, long)]
    /// Asks to list available chains.
    list_chains: bool,

    #[clap(short, long, default_value = "both")]
    /// Configures where the logs should print [default: both] [possible values: file, stdout, both]
    log_to: String,

    #[clap(short, long, default_value = "8115")]
    /// Replaces CKB P2P port in the created config file [default: 8115]
    pub p2p_port: u32,

    #[clap(short, long, default_value = "8114")]
    /// Replaces CKB RPC port in the created config file [default: 8114]
    pub rpc_port: u32,
}

/// replay subcommand
#[derive(Args)]
#[clap()]
pub struct CmdReplay {
    #[clap(short, long, value_name = "Path")]
    /// The directory to store the temporary files
    tmp_target: PathBuf,

    #[clap(long)]
    /// Enable profile on blocks in the range `[from, ..]`
    from: Option<u64>,
    #[clap(long)]
    /// Enable profile on blocks in the range `[.., to]`
    to: Option<u64>,

    #[clap(long)]
    /// Enable sanity check.
    sanity_check: bool,

    #[clap(long)]
    /// Enable full verification.
    full_verification: bool,

    #[clap(long)]
    /// Enable profile
    pub profile: bool,
}

/// Stats subcommand
#[derive(Args)]
#[clap()]
pub struct CmdStats {
    #[clap(short, long)]
    /// Specifies the starting block number. The default is 1
    pub from: Option<u64>,

    #[clap(short, long)]
    /// Specifies the ending block number. The default is the tip block in the database
    pub to: Option<u64>,
}

/// list-hashes subcommand
#[derive(Args)]
#[clap()]
pub struct CmdListHashes {
    #[clap(short, long)]
    /// Lists hashes of the bundled chain specs instead of the current effective one
    pub bundled: bool,
}

/// reset-data subcommand
#[derive(Args)]
#[clap()]
pub struct CmdResetData {
    #[clap(short, long)]
    /// Reset without asking for user confirmation.
    pub force: bool,

    #[clap(long)]
    /// Delete the whole data directory
    pub all: bool,

    #[clap(long)]
    /// Delete only `data/db`
    pub database: bool,

    #[clap(long, long_help = "Delete both peer store and secret key")]
    /// Reset all network data, including the secret key and peer store.
    pub network: bool,

    #[clap(long, long_help = "Delete only `data/network/peer_store`")]
    /// Reset network peer store.
    pub network_peer_store: bool,

    #[clap(long, long_help = "Delete only `data/network/secret_key`")]
    /// Reset network secret key.
    pub network_secret_key: bool,

    #[clap(long, long_help = "Delete only `data/logs`")]
    /// Clean logs directory.
    pub logs: bool,
}

/// peer-id subcommand
#[derive(Subcommand)]
#[clap(about = "About peer id, base on Secp256k1")]
pub enum PeeridSubCommand {
    /// gen subcommand
    Gen(GenSecret),
    /// from-sceret subcommand
    FromSecret(FromSecret),
}

/// gen subcommand
#[derive(Args)]
#[clap(about = "Generate random key to file")]
pub struct GenSecret {
    #[clap(long, long_help = "Generate peer id to file path", value_name = "File")]
    secret_path: PathBuf,
}

/// from-secret subcommand
#[derive(Args)]
#[clap(about = "Generate peer id from secret file")]
pub struct FromSecret {
    #[clap(
        long,
        long_help = "Generate peer id from secret file path",
        value_name = "File"
    )]
    secret_path: PathBuf,
}

/// migrate subcommand
#[derive(Args)]
#[clap()]
pub struct CmdMigrate {
    #[clap(
        long,
        long_help = "Perform database version check without migrating, if migration is in need ExitCode(0) is returned，otherwise ExitCode(64) is returned"
    )]
    /// Check whether it is required to do migration instead of really perform the migration.
    pub check: bool,

    #[clap(long)]
    /// Do migration without interactive prompt.
    pub force: bool,
}

/// db-repair subcommand
#[derive(Args)]
#[clap()]
pub struct CmdDbRepair {}

/// A struct including all the information to start the ckb process.
pub struct Setup {
    /// Subcommand name.
    ///
    /// For example, this is set to `run` when ckb is executed with `ckb run`.
    pub subcommand_name: String,
    /// The config file for the current subcommand.
    pub config: AppConfig,
    /// Whether sentry is enabled.
    #[cfg(feature = "with_sentry")]
    pub is_sentry_enabled: bool,
}

impl Setup {
    /// Boots the ckb process by parsing the command line arguments and loading the config file.
    pub fn from_matches(
        bin_name: String,
        subcommand_name: &str,
        matches: &CkbCli,
    ) -> Result<Setup, ExitCode> {
        let root_dir = Self::root_dir_from_matches(&matches.config)?;
        let mut config = AppConfig::load_for_subcommand(&root_dir, subcommand_name)?;
        config.set_bin_name(bin_name);
        #[cfg(feature = "with_sentry")]
        let is_sentry_enabled = is_daemon(subcommand_name) && config.sentry().is_enabled();

        Ok(Setup {
            subcommand_name: subcommand_name.to_string(),
            config,
            #[cfg(feature = "with_sentry")]
            is_sentry_enabled,
        })
    }

    /// Executes `ckb run`.
    pub fn run(self, matches: &CmdRun) -> Result<RunArgs, ExitCode> {
        let consensus = self.consensus()?;
        let chain_spec_hash = self.chain_spec()?.hash;
        let mut config = self.config.into_ckb()?;

        let mainnet_genesis = ckb_chain_spec::ChainSpec::load_from(
            &ckb_resource::Resource::bundled("specs/mainnet.toml".to_string()),
        )
        .expect("load mainnet spec fail")
        .build_genesis()
        .expect("build mainnet genesis fail");
        config.network.sync.min_chain_work =
            if consensus.genesis_block.hash() == mainnet_genesis.hash() {
                MIN_CHAIN_WORK_500K
            } else {
                u256!("0x0")
            };

        config.network.sync.assume_valid_target = matches
            .assume_valid_target
            .as_ref()
            .and_then(|s| H256::from_str(&s[2..]).ok());

        Ok(RunArgs {
            config,
            consensus,
            block_assembler_advanced: matches.ba_advanced,
            skip_chain_spec_check: matches.skip_spec_check,
            overwrite_chain_spec: matches.overwrite_spec,
            chain_spec_hash,
        })
    }

    /// `migrate` subcommand has one `flags` arg, trigger this arg with "--check"
    pub fn migrate(self, matches: &CmdMigrate) -> Result<MigrateArgs, ExitCode> {
        let consensus = self.consensus()?;
        let config = self.config.into_ckb()?;
        let check = matches.check;
        let force = matches.force;

        Ok(MigrateArgs {
            config,
            consensus,
            check,
            force,
        })
    }

    /// `db-repair` subcommand
    pub fn db_repair(self, _matches: &CmdDbRepair) -> Result<RepairArgs, ExitCode> {
        let config = self.config.into_ckb()?;

        Ok(RepairArgs { config })
    }

    /// Executes `ckb miner`.
    pub fn miner(self, matches: &CmdMiner) -> Result<MinerArgs, ExitCode> {
        let spec = self.chain_spec()?;
        let memory_tracker = self.config.memory_tracker().to_owned();
        let config = self.config.into_miner()?;
        let pow_engine = spec.pow_engine();

        Ok(MinerArgs {
            pow_engine,
            config: config.miner,
            memory_tracker,
            limit: matches.limit,
        })
    }

    /// Executes `ckb replay`.
    pub fn replay(self, matches: &CmdReplay) -> Result<ReplayArgs, ExitCode> {
        let consensus = self.consensus()?;
        let config = self.config.into_ckb()?;
        let tmp_target = matches.tmp_target.clone();
        let profile = if matches.profile {
            let from = matches.from;
            let to = matches.to;
            Some((from, to))
        } else {
            None
        };
        let sanity_check = matches.sanity_check;
        let full_verification = matches.full_verification;
        Ok(ReplayArgs {
            config,
            consensus,
            tmp_target,
            profile,
            sanity_check,
            full_verification,
        })
    }

    /// Executes `ckb stats`.
    pub fn stats(self, matches: &CmdStats) -> Result<StatsArgs, ExitCode> {
        let consensus = self.consensus()?;
        let config = self.config.into_ckb()?;

        let from = matches.from;
        let to = matches.to;

        Ok(StatsArgs {
            config,
            consensus,
            from,
            to,
        })
    }

    /// Executes `ckb import`.
    pub fn import(self, matches: &CmdImport) -> Result<ImportArgs, ExitCode> {
        let consensus = self.consensus()?;
        let config = self.config.into_ckb()?;
        let source = matches.source.clone();

        Ok(ImportArgs {
            config,
            consensus,
            source,
        })
    }

    /// Executes `ckb export`.
    pub fn export(self, matches: &CmdExport) -> Result<ExportArgs, ExitCode> {
        let consensus = self.consensus()?;
        let config = self.config.into_ckb()?;

        Ok(ExportArgs {
            config,
            consensus,
            target: matches.target.clone(),
        })
    }

    /// Executes `ckb init`.
    pub fn init(matches: &CmdInit) -> Result<InitArgs, ExitCode> {
        // if matches.is_present("list-specs") {
        //     eprintln!(
        //         "Deprecated: Option `--list-specs` is deprecated, use `--list-chains` instead"
        //     );
        // }
        // if matches.is_present("spec") {
        //     eprintln!("Deprecated: Option `--spec` is deprecated, use `--chain` instead");
        // }
        // if matches.is_present("export-specs") {
        //     eprintln!("Deprecated: Option `--export-specs` is deprecated");
        // }

        let root_dir = Self::root_dir_from_matches(&matches.config)?;
        let list_chains = matches.list_chains;
        let interactive = matches.interactive;
        let force = matches.force;

        // --import-spec override --chain
        let chain = {
            if matches.import_spec.is_none() {
                matches.chain.clone()
            } else {
                matches.import_spec.clone().unwrap()
            }
        };
        let rpc_port = matches.rpc_port.to_string();
        let p2p_port = matches.p2p_port.to_string();
        let (log_to_file, log_to_stdout) = match matches.log_to.as_str() {
            "file" => (true, false),
            "stdout" => (false, true),
            "both" => (true, true),
            _ => unreachable!(),
        };

        let block_assembler_code_hash = matches.ba_code_hash.clone();
        let block_assembler_args: Vec<_> = matches.ba_arg.clone().unwrap_or_default();
        let block_assembler_hash_type =
            serde_plain::from_str::<ScriptHashType>(&matches.ba_hash_type).unwrap();
        let block_assembler_message = matches.ba_message.clone();

        let import_spec = matches.import_spec.clone();

        let customize_spec = {
            let genesis_message = matches.genesis_message.clone();
            args::CustomizeSpec { genesis_message }
        };

        Ok(InitArgs {
            interactive,
            root_dir,
            chain,
            rpc_port,
            p2p_port,
            list_chains,
            force,
            log_to_file,
            log_to_stdout,
            block_assembler_code_hash,
            block_assembler_args,
            block_assembler_hash_type,
            block_assembler_message,
            import_spec,
            customize_spec,
        })
    }

    /// Executes `ckb reset-data`.
    pub fn reset_data(self, matches: &CmdResetData) -> Result<ResetDataArgs, ExitCode> {
        let config = self.config.into_ckb()?;
        let data_dir = config.data_dir;
        let db_path = config.db.path;
        let network_config = config.network;
        let network_dir = network_config.path.clone();
        let network_peer_store_path = network_config.peer_store_path();
        let network_secret_key_path = network_config.secret_key_path();
        let logs_dir = Some(config.logger.log_dir);

        let force = matches.force;
        let all = matches.all;
        let database = matches.database;
        let network = matches.network;
        let network_peer_store = matches.network_peer_store;
        let network_secret_key = matches.network_secret_key;
        let logs = matches.logs;

        Ok(ResetDataArgs {
            force,
            all,
            database,
            network,
            network_peer_store,
            network_secret_key,
            logs,
            data_dir,
            db_path,
            network_dir,
            network_peer_store_path,
            network_secret_key_path,
            logs_dir,
        })
    }

    /// Resolves the root directory for ckb from the command line arguments.
    pub fn root_dir_from_matches(config: &Option<PathBuf>) -> Result<PathBuf, ExitCode> {
        let config_dir = match config {
            Some(arg_config_dir) => PathBuf::from(arg_config_dir),
            None => ::std::env::current_dir()?,
        };
        std::fs::create_dir_all(&config_dir)?;
        Ok(config_dir)
    }

    /// Loads the chain spec.
    #[cfg(feature = "with_sentry")]
    fn chain_spec(&self) -> Result<ChainSpec, ExitCode> {
        let result = self.config.chain_spec();
        if let Ok(spec) = &result {
            if self.is_sentry_enabled {
                sentry::configure_scope(|scope| {
                    scope.set_tag("spec.name", &spec.name);
                    scope.set_tag("spec.pow", &spec.pow);
                });
            }
        }

        result
    }

    #[cfg(not(feature = "with_sentry"))]
    fn chain_spec(&self) -> Result<ChainSpec, ExitCode> {
        self.config.chain_spec()
    }

    /// Gets the consensus.
    #[cfg(feature = "with_sentry")]
    pub fn consensus(&self) -> Result<Consensus, ExitCode> {
        let result = consensus_from_spec(&self.chain_spec()?);

        if let Ok(consensus) = &result {
            if self.is_sentry_enabled {
                sentry::configure_scope(|scope| {
                    scope.set_tag("genesis", consensus.genesis_hash());
                });
            }
        }

        result
    }

    /// Gets the consensus.
    #[cfg(not(feature = "with_sentry"))]
    pub fn consensus(&self) -> Result<Consensus, ExitCode> {
        consensus_from_spec(&self.chain_spec()?)
    }

    /// Gets the network peer id by reading the network secret key.
    pub fn peer_id(matches: &FromSecret) -> Result<PeerIDArgs, ExitCode> {
        let path = matches.secret_path.clone();
        match read_secret_key(path) {
            Ok(Some(key)) => Ok(PeerIDArgs {
                peer_id: key.peer_id(),
            }),
            Err(_) => Err(ExitCode::Failure),
            Ok(None) => Err(ExitCode::IO),
        }
    }

    /// Generates the network secret key.
    pub fn gen(matches: &GenSecret) -> Result<(), ExitCode> {
        let path = matches.secret_path.clone();
        configs::write_secret_to_file(&configs::generate_random_key(), path)
            .map_err(|_| ExitCode::IO)
    }
}

// There are two types of errors,
// parse failures and those where the argument wasn't present
#[doc(hidden)]
#[macro_export]
macro_rules! option_value_t {
    ($m:ident, $v:expr, $t:ty) => {
        option_value_t!($m.value_of($v), $t)
    };
    ($m:ident.value_of($v:expr), $t:ty) => {
        match $m.value_of_t($v) {
            Ok(from) => Ok(Some(from)),
            Err(ref e) if e.kind() == ErrorKind::ArgumentNotFound => Ok(None),
            Err(e) => Err(e),
        }
    };
}

#[cfg(feature = "with_sentry")]
fn is_daemon(subcommand_name: &str) -> bool {
    matches!(subcommand_name, CMD_RUN | CMD_MINER)
}

fn consensus_from_spec(spec: &ChainSpec) -> Result<Consensus, ExitCode> {
    spec.build_consensus().map_err(|err| {
        eprintln!("chainspec error: {}", err);
        ExitCode::Config
    })
}

/// validator to check if Hash256 format
fn is_hash256(hex: &str) -> Result<(), String> {
    let tmp = hex.as_bytes();
    if tmp[..2] == b"0x"[..] {
        match H256::from_slice(&tmp[2..]) {
            Ok(_) => Ok(()),
            _ => Err("input string is not valid H256 format".to_owned()),
        }
    } else {
        Err("Must be a 0x-prefixed hexadecimal string".to_owned())
    }
}
