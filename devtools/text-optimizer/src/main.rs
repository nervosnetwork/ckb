mod backfill;
mod error;
mod extractors;
mod types;
mod yaml_processor;

use backfill::backfill;
use clap::{Parser, Subcommand};
use error::MyError;
use extractors::extract;
use std::path::PathBuf;
use std::process::Command;
use std::result::Result;

pub const GITHUB_REPO: &str = "https://github.com/nervosnetwork/ckb/tree";
pub const PROJECT_ROOT_CARGO_CONFIG_PATH: &str = "../../Cargo.toml";
pub const LOG_TEXT_FILE: &str = "log_text_list.yml";
pub const CLAP_TEXT_FILE: &str = "clap_text_list.yml";
pub const STD_OUTPUT_TEXT_FILE: &str = "std_output_text_list.yml";
pub const THISERROR_TEXT_FILE: &str = "thiserror_text_list.yml";

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// extracting text
    Extract {
        /// specify a github commit id as the rev for generating the source link
        #[arg(short, long)]
        commit_id: String,

        /// specifies a directory path for .yml text files output
        #[arg(short, long, default_value = PathBuf::from("./").into_os_string())]
        output_dir: PathBuf,
    },
    /// backfill text
    Backfill {
        /// specifies a directory path for .yml text files input
        #[arg(short, long, default_value = PathBuf::from("./").into_os_string())]
        input_dir: PathBuf,
    },
}

fn main() {
    let cli = Cli::parse();

    env_logger::init_from_env(
        env_logger::Env::default().filter_or(env_logger::DEFAULT_FILTER_ENV, "info"),
    );

    match &cli.command {
        Some(Commands::Extract {
            commit_id,
            output_dir,
        }) => {
            check_commit_id(commit_id).expect("check commit id");
            extract(
                PathBuf::from(PROJECT_ROOT_CARGO_CONFIG_PATH),
                commit_id,
                output_dir,
            );
        }
        Some(Commands::Backfill { input_dir }) => {
            backfill(input_dir);
        }
        None => {}
    }
}

fn check_commit_id(commit_id: &str) -> Result<(), MyError> {
    let output = Command::new("git")
        .arg("rev-parse")
        .arg("HEAD")
        .output()
        .expect("Failed to execute git command");

    if output.status.success() {
        let current_commit_id = String::from_utf8_lossy(&output.stdout);
        let current_commit_id: String = current_commit_id.trim().chars().take(7).collect();
        let commit_id: String = commit_id.trim().chars().take(7).collect();
        if current_commit_id == commit_id {
            log::info!(
                "Current commit ID matches the expected commit ID: {}",
                current_commit_id
            );
            Ok(())
        } else {
            log::warn!(
                "Current commit ID {} does not match the expected commit ID {}.",
                current_commit_id,
                commit_id
            );
            Err(MyError::CommitId)
        }
    } else {
        log::error!("Failed to retrieve the current commit ID");
        Err(MyError::CommitId)
    }
}
