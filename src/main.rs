mod helper;
mod subcommand;

use build_info::Version;
use ckb_app_config::{cli, ExitCode, Setup};

#[cfg(unix)]
#[global_allocator]
static ALLOC: jemallocator::Jemalloc = jemallocator::Jemalloc;

fn run_app() -> Result<(), ExitCode> {
    // Always print backtrace on panic.
    ::std::env::set_var("RUST_BACKTRACE", "full");

    let version = get_version();
    let app_matches = cli::get_matches(&version);
    match app_matches.subcommand() {
        (cli::CMD_INIT, Some(matches)) => return subcommand::init(Setup::init(&matches)?),
        (cli::CMD_CLI, Some(matches)) => {
            return match matches.subcommand() {
                (cli::CMD_BLAKE160, Some(sub_matches)) => subcommand::cli::blake160(sub_matches),
                (cli::CMD_BLAKE256, Some(sub_matches)) => subcommand::cli::blake256(sub_matches),
                (cli::CMD_SECP256K1_LOCK, Some(sub_matches)) => {
                    subcommand::cli::secp256k1_lock(sub_matches)
                }
                (cli::CMD_HASHES, Some(sub_matches)) => {
                    subcommand::cli::hashes(Setup::locator_from_matches(&matches)?, sub_matches)
                }
                _ => unreachable!(),
            };
        }
        _ => {
            // continue
        }
    }

    let setup = Setup::from_matches(&app_matches)?;
    let _guard = setup.setup_app(&version);

    match app_matches.subcommand() {
        (cli::CMD_RUN, _) => subcommand::run(setup.run()?, version),
        (cli::CMD_MINER, _) => subcommand::miner(setup.miner()?),
        (cli::CMD_PROF, Some(matches)) => subcommand::profile(setup.prof(&matches)?),
        (cli::CMD_EXPORT, Some(matches)) => subcommand::export(setup.export(&matches)?),
        (cli::CMD_IMPORT, Some(matches)) => subcommand::import(setup.import(&matches)?),
        _ => unreachable!(),
    }
}

fn main() {
    if let Some(exit_code) = run_app().err() {
        ::std::process::exit(exit_code.into());
    }
}

fn get_version() -> Version {
    let major = env!("CARGO_PKG_VERSION_MAJOR")
        .parse::<u8>()
        .expect("CARGO_PKG_VERSION_MAJOR parse success");
    let minor = env!("CARGO_PKG_VERSION_MINOR")
        .parse::<u8>()
        .expect("CARGO_PKG_VERSION_MINOR parse success");
    let patch = env!("CARGO_PKG_VERSION_PATCH")
        .parse::<u16>()
        .expect("CARGO_PKG_VERSION_PATCH parse success");
    let dash_pre = {
        let pre = env!("CARGO_PKG_VERSION_PRE");
        if pre == "" {
            pre.to_string()
        } else {
            "-".to_string() + pre
        }
    };

    let commit_describe = option_env!("COMMIT_DESCRIBE").map(ToString::to_string);
    #[cfg(docker)]
    let commit_describe = commit_describe.map(|s| s.replace("-dirty", ""));
    let commit_date = option_env!("COMMIT_DATE").map(ToString::to_string);
    Version {
        major,
        minor,
        patch,
        dash_pre,
        commit_describe,
        commit_date,
    }
}
