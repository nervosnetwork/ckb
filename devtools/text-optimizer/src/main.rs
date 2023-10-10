mod backfill;
mod extractors;
mod types;
mod yaml_processor;

use backfill::backfill;
use clap::{Parser, Subcommand};
use extractors::extract;
use std::path::PathBuf;

pub const PROJECT_ROOT: &str = "../../Cargo.toml";
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
        Some(Commands::Extract { output_dir }) => {
            extract(PathBuf::from(PROJECT_ROOT), output_dir);
        }
        Some(Commands::Backfill { input_dir }) => {
            backfill(input_dir);
        }
        None => {}
    }
}
