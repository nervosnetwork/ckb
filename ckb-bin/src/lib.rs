//! CKB executable.
//!
//! This crate is created to reduce the link time to build CKB.
use clap::Parser;

use ckb_app_config::{CKBSubCommand, CkbCli, ExitCode, PeeridSubCommand, Setup};
use ckb_async_runtime::new_global_runtime;
use ckb_build_info::Version;
use helper::raise_fd_limit;
use setup_guard::SetupGuard;

mod helper;
mod setup_guard;
mod subcommand;

#[cfg(feature = "with_sentry")]
pub(crate) const LOG_TARGET_SENTRY: &str = "sentry";

const BIN_NAME: &str = "ckb";

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

    let cli = CkbCli::parse();

    // process these three subcommands(without ckb configuration)
    match &cli.sub_command {
        CKBSubCommand::Init(matches) => return subcommand::init(Setup::init(matches)?),
        CKBSubCommand::ListHashes(matches) => {
            return subcommand::list_hashes(Setup::root_dir_from_matches(&cli.config)?, matches)
        }
        CKBSubCommand::PeerId(matches) => match matches {
            PeeridSubCommand::Gen(matches) => return Setup::gen(matches),
            PeeridSubCommand::FromSecret(matches) => {
                return subcommand::peer_id(Setup::peer_id(matches)?)
            }
        },
        _ => {}
    };

    let is_silent_logging = is_silent_logging(&cli.sub_command);
    let (handle, mut rt_stop) = new_global_runtime();
    let setup = Setup::from_matches(BIN_NAME.to_owned(), &cli.sub_command.to_string(), &cli)?;
    let _guard = SetupGuard::from_setup(&setup, &version, handle.clone(), is_silent_logging)?;

    raise_fd_limit();

    let ret = match &cli.sub_command {
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

    rt_stop.try_send(());
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
