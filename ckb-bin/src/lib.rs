//! CKB executable.
//!
//! This crate is created to reduce the link time to build CKB.
mod helper;
mod setup_guard;
mod subcommand;

use ckb_app_config::{cli, ExitCode, Setup};
use ckb_async_runtime::new_global_runtime;
use ckb_build_info::Version;
use ckb_logger::{debug, info};
use ckb_network::tokio;
use helper::raise_fd_limit;
use setup_guard::SetupGuard;

#[cfg(feature = "with_sentry")]
pub(crate) const LOG_TARGET_SENTRY: &str = "sentry";

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

    let (bin_name, app_matches) = cli::get_bin_name_and_matches(&version);
    if let Some((cli, matches)) = app_matches.subcommand() {
        match cli {
            cli::CMD_INIT => {
                return subcommand::init(Setup::init(matches)?);
            }
            cli::CMD_LIST_HASHES => {
                return subcommand::list_hashes(Setup::root_dir_from_matches(matches)?, matches);
            }
            cli::CMD_PEERID => {
                if let Some((cli, matches)) = matches.subcommand() {
                    match cli {
                        cli::CMD_GEN_SECRET => return Setup::gen(matches),
                        cli::CMD_FROM_SECRET => {
                            return subcommand::peer_id(Setup::peer_id(matches)?)
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    let (cmd, matches) = app_matches
        .subcommand()
        .expect("SubcommandRequiredElseHelp");
    let is_silent_logging = is_silent_logging(cmd);

    let (mut handle, mut handle_stop_rx, _runtime) = new_global_runtime();
    let setup = Setup::from_matches(bin_name, cmd, matches)?;
    let _guard = SetupGuard::from_setup(&setup, &version, handle.clone(), is_silent_logging)?;

    raise_fd_limit();

    let ret = match cmd {
        cli::CMD_RUN => subcommand::run(setup.run(matches)?, version, handle.clone()),
        cli::CMD_MINER => subcommand::miner(setup.miner(matches)?, handle.clone()),
        cli::CMD_REPLAY => subcommand::replay(setup.replay(matches)?, handle.clone()),
        cli::CMD_EXPORT => subcommand::export(setup.export(matches)?, handle.clone()),
        cli::CMD_IMPORT => subcommand::import(setup.import(matches)?, handle.clone()),
        cli::CMD_STATS => subcommand::stats(setup.stats(matches)?, handle.clone()),
        cli::CMD_RESET_DATA => subcommand::reset_data(setup.reset_data(matches)?),
        cli::CMD_MIGRATE => subcommand::migrate(setup.migrate(matches)?),
        _ => unreachable!(),
    };

    if matches!(cmd, cli::CMD_RUN) {
        handle.drop_guard();

        tokio::task::block_in_place(|| {
            debug!("waiting all tokio tasks done");
            handle_stop_rx.blocking_recv();
            info!("ckb shutdown");
        });
    }

    ret
}

type Silent = bool;

fn is_silent_logging(cmd: &str) -> Silent {
    matches!(
        cmd,
        cli::CMD_EXPORT | cli::CMD_IMPORT | cli::CMD_STATS | cli::CMD_MIGRATE | cli::CMD_RESET_DATA
    )
}
