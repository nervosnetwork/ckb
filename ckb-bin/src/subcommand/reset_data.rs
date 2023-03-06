use std::fs;

use crate::helper::prompt;
use ckb_app_config::{ExitCode, ResetDataArgs};

pub fn reset_data(args: ResetDataArgs) -> Result<(), ExitCode> {
    let mut target_dirs = vec![];
    let mut target_files = vec![];
    let mut errors_count = 0;

    if args.all {
        target_dirs.push(args.data_dir);
    }

    if args.database {
        target_dirs.push(args.db_path);
    }

    if args.network {
        target_dirs.push(args.network_dir);
    }

    if args.network_peer_store {
        target_dirs.push(args.network_peer_store_path);
    }

    if args.network_secret_key {
        target_files.push(args.network_secret_key_path);
    }

    if args.logs {
        if let Some(dir) = args.logs_dir {
            target_dirs.push(dir);
        }
    }

    if !args.force && (!target_dirs.is_empty() || !target_files.is_empty()) {
        let to_be_deleted_targets = target_dirs
            .iter()
            .chain(target_files.iter())
            .map(|path| path.to_string_lossy())
            .collect::<Vec<_>>()
            .join(", ");

        let input = prompt(format!("remove {to_be_deleted_targets}? ").as_str());
        if !["y", "Y"].contains(&input.trim()) {
            return Ok(());
        }
    }

    for dir in target_dirs.iter() {
        if dir.exists() {
            println!("deleting {}", dir.display());
            if let Some(e) = fs::remove_dir_all(dir).err() {
                eprintln!("{e}");
                errors_count += 1;
            }
        }
    }

    for file in target_files.iter() {
        if file.exists() {
            println!("deleting {}", file.display());
            if let Some(e) = fs::remove_file(file).err() {
                eprintln!("{e}");
                errors_count += 1;
            }
        }
    }

    if errors_count == 0 {
        Ok(())
    } else {
        Err(ExitCode::Failure)
    }
}
