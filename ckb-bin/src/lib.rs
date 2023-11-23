//! CKB executable.
//!
//! This crate is created to reduce the link time to build CKB.
mod helper;
mod setup_guard;
mod subcommand;
use ckb_app_config::{cli, ExitCode, Setup};
use ckb_async_runtime::new_global_runtime;
use ckb_build_info::Version;
use ckb_logger::info;
use ckb_network::tokio;
use clap::ArgMatches;
use colored::Colorize;
use daemonize::Daemonize;
use helper::raise_fd_limit;
use setup_guard::SetupGuard;

#[cfg(not(target_os = "windows"))]
use subcommand::check_process;

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

    if run_deamon(cmd, matches) {
        run_in_daemon(version, bin_name, cmd, matches)
    } else {
        debug!("ckb version: {}", version);
        run_app_inner(version, bin_name, cmd, matches)
    }
}

fn run_in_daemon(
    version: Version,
    bin_name: String,
    cmd: &str,
    matches: &ArgMatches,
) -> Result<(), ExitCode> {
    eprintln!("starting CKB in daemon mode ...");
    eprintln!("check status : `{}`", "ckb daemon --check".green());
    eprintln!("stop daemon  : `{}`", "ckb daemon --stop".yellow());

    assert!(matches!(cmd, cli::CMD_RUN));
    let root_dir = Setup::root_dir_from_matches(matches)?;
    let daemon_dir = root_dir.join("data/daemon");
    let pid_file = Setup::pid_file_path_from_matches(matches)?;

    if check_process(&pid_file).is_ok() {
        eprintln!("{}", "ckb is already running".red());
        return Ok(());
    }
    eprintln!("no ckb process, starting ...");

    // make sure daemon dir exists
    std::fs::create_dir_all(daemon_dir)?;

    let pwd = std::env::current_dir()?;
    let daemon = Daemonize::new()
        .pid_file(pid_file)
        .chown_pid_file(true)
        .working_directory(pwd);

    match daemon.start() {
        Ok(_) => {
            info!("Success, daemonized ...");
            run_app_inner(version, bin_name, cmd, matches)
        }
        Err(e) => {
            info!("daemonize error: {}", e);
            Err(ExitCode::Failure)
        }
    }
}

fn run_app_inner(
    version: Version,
    bin_name: String,
    cmd: &str,
    matches: &ArgMatches,
) -> Result<(), ExitCode> {
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
        #[cfg(not(target_os = "windows"))]
        cli::CMD_DAEMON => subcommand::daemon(setup.daemon(matches)?),
        _ => unreachable!(),
    };

    if matches!(cmd, cli::CMD_RUN) {
        handle.drop_guard();

        tokio::task::block_in_place(|| {
            info!("Waiting for all tokio tasks to exit...");
            handle_stop_rx.blocking_recv();
            info!("All tokio tasks and threads have exited. CKB shutdown");
        });
    }

    ret
}

fn run_deamon(cmd: &str, matches: &ArgMatches) -> bool {
    #[cfg(target_os = "windows")]
    return false;

    match cmd {
        cli::CMD_RUN => matches.get_flag(cli::ARG_DAEMON),
        _ => false,
    }
}

type Silent = bool;

fn is_silent_logging(cmd: &str) -> Silent {
    matches!(
        cmd,
        cli::CMD_EXPORT | cli::CMD_IMPORT | cli::CMD_STATS | cli::CMD_MIGRATE | cli::CMD_RESET_DATA
    )
}
