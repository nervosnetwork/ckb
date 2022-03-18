//! CKB executable.
//!
//! This crate is created to reduce the link time to build CKB.

use std::path::PathBuf;
use std::time::Duration;

use clap::FromArgMatches;

use ckb_app_config::{
    basic_app, CKBSubCommand, ExitCode, PeeridSubCommand, Setup, ARG_CONFIG_DIR, BIN_NAME,
};
use ckb_async_runtime::new_global_runtime;
use ckb_build_info::Version;
use helper::raise_fd_limit;
use setup_guard::SetupGuard;

mod helper;
mod setup_guard;
mod subcommand;

#[cfg(feature = "with_sentry")]
pub(crate) const LOG_TARGET_SENTRY: &str = "sentry";
const RUNTIME_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(1);

/// The executable main entry.
///
/// It returns `Ok` when the process exist normally, otherwise the `ExitCode` is converted to the
/// process exit status code.
///
/// ## Parameters
///
/// * `version` - The version is passed in so the bin crate can collect the version without trigger
/// re-linking.
pub fn run_app(version: Version) -> Result<(), ExitCode> {
    // Always print backtrace on panic.
    ::std::env::set_var("RUST_BACKTRACE", "full");

    let bin_name = std::env::args()
        .next()
        .unwrap_or_else(|| BIN_NAME.to_owned());
    let long = version.long();
    let short = version.short();

    let cli = basic_app()
        .version(short.as_str())
        .long_version(long.as_str());

    let matches = cli.get_matches();
    let derived_subcommands = CKBSubCommand::from_arg_matches(&matches)?;

    // process these three subcommands(without ckb configuration)
    match &derived_subcommands {
        CKBSubCommand::Init(matches) => return subcommand::init(Setup::init(matches)?),
        CKBSubCommand::ListHashes(list_hash_matches) => {
            let cfg = matches.value_of(ARG_CONFIG_DIR).map(PathBuf::from);
            return subcommand::list_hashes(Setup::root_dir_from_matches(&cfg)?, list_hash_matches);
        }
        CKBSubCommand::PeerId(matches) => match matches {
            PeeridSubCommand::Gen(matches) => return Setup::gen(matches),
            PeeridSubCommand::FromSecret(matches) => {
                return subcommand::peer_id(Setup::peer_id(matches)?)
            }
        },
        _ => {}
    };

    let is_silent_logging = is_silent_logging(&derived_subcommands);
    let (handle, runtime) = new_global_runtime();
    let setup = Setup::from_matches(bin_name, &derived_subcommands.to_string(), &matches)?;
    let _guard = SetupGuard::from_setup(&setup, &version, handle.clone(), is_silent_logging)?;

    raise_fd_limit();

    let ret = match &derived_subcommands {
        CKBSubCommand::Run(matches) => subcommand::run(setup.run(matches)?, version, handle),
        CKBSubCommand::Miner(matches) => subcommand::miner(setup.miner(matches)?, handle),
        CKBSubCommand::Replay(matches) => subcommand::replay(setup.replay(matches)?, handle),
        CKBSubCommand::Export(matches) => subcommand::export(setup.export(matches)?, handle),
        CKBSubCommand::Import(matches) => subcommand::import(setup.import(matches)?, handle),
        CKBSubCommand::Stats(matches) => subcommand::stats(setup.stats(matches)?, handle),
        CKBSubCommand::ResetData(matches) => subcommand::reset_data(setup.reset_data(matches)?),
        CKBSubCommand::Migrate(matches) => subcommand::migrate(setup.migrate(matches)?),
        CKBSubCommand::DbRepair(matches) => subcommand::db_repair(setup.db_repair(matches)?),
        _ => unreachable!(),
    };

    runtime.shutdown_timeout(RUNTIME_SHUTDOWN_TIMEOUT);
    ret
}

type Silent = bool;

fn is_silent_logging(cmd: &CKBSubCommand) -> Silent {
    matches!(
        cmd,
        CKBSubCommand::Export(_)
            | CKBSubCommand::Import(_)
            | CKBSubCommand::Stats(_)
            | CKBSubCommand::Migrate(_)
            | CKBSubCommand::DbRepair(_)
            | CKBSubCommand::ResetData(_)
    )
}
