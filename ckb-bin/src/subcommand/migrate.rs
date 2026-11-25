use ckb_app_config::{ExitCode, MigrateArgs};
use ckb_db::{RocksDB, Schema};
use ckb_error::{Error, InternalErrorKind};
use ckb_migrate::migrate::Migrate;
use ckb_migrate::sst_rebuild::format_bytes;
use ckb_resource::{DB_OPTIONS_FILE_NAME, Resource, TemplateContext};
use is_terminal::IsTerminal;
use std::cmp::Ordering;
use std::fs::{self, OpenOptions};
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::helper::prompt;

pub fn migrate(args: MigrateArgs) -> Result<(), ExitCode> {
    let effective_options_file = effective_sst_rebuild_options_file(
        &args.config.root_dir,
        args.config.db.options_file.as_deref(),
    );
    let migrate = Migrate::new(&args.config.db.path, args.consensus.hardfork_switch)
        .with_options_file(effective_options_file);

    if args.sst_rebuild {
        let recovering = Migrate::has_pending_sst_rebuild(&args.config.db.path);
        let mut allow_insufficient_space = false;

        if !args.force {
            if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
                if recovering {
                    let input = prompt(
                        "\
                        \n\
                        An interrupted SST rebuild migration was detected.\n\
                        Continuing will complete the cutover and refresh default.db-options if needed.\n\
                        \nIf you want to recover the migration, please input YES, otherwise, the current process will exit.\n\
                        > ",
                    );
                    if input.trim().to_lowercase() != "yes" {
                        eprintln!("Migration was declined since the user didn't confirm.");
                        return Err(ExitCode::Failure);
                    }
                } else {
                    migrate.preflight_sst_rebuild().map_err(|err| {
                        eprintln!("Run error: {err:?}");
                        ExitCode::Failure
                    })?;

                    let input = prompt(
                        "\
                        \n\
                        SST rebuild migration will create a new RocksDB database beside the current one,\n\
                        ingest generated SST files, validate the result, and then move the current DB to a backup path.\n\
                        \n\
                        The old database is kept as a backup, so free disk space of about 1.1x the\n\
                        current database size is needed to build the new one.\n\
                        \n\
                        CKB must be stopped while this runs. We strongly recommend backing up the data directory first.\n\
                        \nIf you want to rebuild the data, please input YES, otherwise, the current process will exit.\n\
                        > ",
                    );
                    if input.trim().to_lowercase() != "yes" {
                        eprintln!("Migration was declined since the user didn't confirm.");
                        return Err(ExitCode::Failure);
                    }

                    let report = migrate.sst_rebuild_disk_space().map_err(|err| {
                        eprintln!("Run error: {err:?}");
                        ExitCode::Failure
                    })?;
                    let available = report
                        .available
                        .map(format_bytes)
                        .unwrap_or_else(|| "unknown".to_string());
                    eprintln!(
                        "Database {} size {}, required free space {}, available {}",
                        args.config.db.path.display(),
                        format_bytes(report.database_size),
                        format_bytes(report.required),
                        available,
                    );
                    if report.has_minimum_space() == Some(false) {
                        eprintln!(
                            "Not enough free disk space to build the new database: {} is available, but at least {} (the current database size) is needed. The migration would fail; free up space or move the data directory to a larger disk, then retry.",
                            available,
                            format_bytes(report.database_size),
                        );
                        return Err(ExitCode::Failure);
                    }
                    let default_yes = report.has_enough_space() != Some(false);
                    let question = if default_yes {
                        "Continue with the SST rebuild migration? [Y/n] > "
                    } else {
                        "Not enough free disk space; the migration may fail. Continue anyway? [y/N] > "
                    };
                    if !confirm(question, default_yes) {
                        eprintln!("Migration was declined since the user didn't confirm.");
                        return Err(ExitCode::Failure);
                    }
                    allow_insufficient_space = !default_yes;
                }
            } else {
                eprintln!("Run error: use --force with --sst-rebuild without interactive prompt");
                return Err(ExitCode::Failure);
            }
        }

        warn_custom_legacy_db_options(
            &args.config.root_dir,
            args.config.db.options_file.as_deref(),
        );

        let root_dir = &args.config.root_dir;
        let options_file = args.config.db.options_file.as_deref();
        migrate
            .sst_rebuild_with_post_swap(allow_insufficient_space, || {
                refresh_default_db_options_if_needed(root_dir, options_file)
            })
            .map_err(|err| {
                eprintln!("Run error: {err:?}");
                ExitCode::Failure
            })?;
        return Ok(());
    }

    let _sst_rebuild_lock = Migrate::acquire_shared_lock(&args.config.db.path).map_err(|e| {
        eprintln!("Migration error {e}");
        ExitCode::Failure
    })?;

    if Migrate::has_pending_sst_rebuild(&args.config.db.path) {
        if args.check {
            // Return 0 so scripted `if ckb migrate --check; ...` knows action is required.
            return Ok(());
        }
        eprintln!(
            "An interrupted SST rebuild migration was detected for {}. Run `ckb migrate --sst-rebuild` to finish recovery.",
            args.config.db.path.display()
        );
        return Err(ExitCode::Failure);
    }

    {
        let read_only_db = migrate.open_read_only_db().map_err(|e| {
            eprintln!("Migration error {e}");
            ExitCode::Failure
        })?;

        if let Some(db) = read_only_db {
            // if there are only pending background migrations, they will run automatically
            // so here we check with `include_background` as true
            let db_status = migrate.check(&db, true);
            if matches!(db_status, Ordering::Greater) {
                eprintln!(
                    "The database was created by a higher version CKB executable binary \n\
                     and cannot be opened by the current binary.\n\
                     Please download the latest CKB executable binary."
                );
                return Err(ExitCode::Failure);
            }

            // `include_background` is default to false
            let db_status = migrate.check(&db, args.include_background);
            if args.check {
                if matches!(db_status, Ordering::Less) {
                    // special for bash usage, return 0 means need run migration
                    // if ckb migrate --check; then ckb migrate --force; fi
                    return Ok(());
                } else {
                    return Err(ExitCode::Cli);
                }
            }

            if matches!(db_status, Ordering::Equal) {
                return Ok(());
            }

            migrate.check_wal_dir().map_err(|e| {
                eprintln!("Migration error {e}");
                ExitCode::Failure
            })?;

            if migrate.require_expensive(&db, args.include_background) && !args.force {
                if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
                    let input = prompt(
                        "\
                    \n\
                    Doing migration will take quite a long time before CKB could work again.\n\
                    \n\
                    Once the migration started, the data will be no longer compatible with all older versions CKB,\n\
                    so we strongly recommended you to backup the old data before migrating.\n\
                    \n\
                    If the migration failed, try to delete all data and sync from scratch.\n\
                    \nIf you want to migrate the data, please input YES, otherwise, the current process will exit.\n\
                    > ",
                    );
                    if input.trim().to_lowercase() != "yes" {
                        eprintln!("Migration was declined since the user didn't confirm.");
                        return Err(ExitCode::Failure);
                    }
                } else {
                    eprintln!("Run error: use --force to migrate without interactive prompt");
                    return Err(ExitCode::Failure);
                }
            }
        }
    }

    let bulk_load_db_db = migrate.open_bulk_load_db().map_err(|e| {
        eprintln!("Migration error {e}");
        ExitCode::Failure
    })?;

    if let Some(db) = bulk_load_db_db {
        migrate.migrate(db, false).map_err(|err| {
            eprintln!("Run error: {err:?}");
            ExitCode::Failure
        })?;
    }
    Ok(())
}

fn confirm(msg: &str, default_yes: bool) -> bool {
    loop {
        match prompt(msg).trim().to_lowercase().as_str() {
            "" => return default_yes,
            "y" | "yes" => return true,
            "n" | "no" => return false,
            _ => eprintln!("Please answer yes or no."),
        }
    }
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => match normalized.components().next_back() {
                Some(Component::Normal(_)) => {
                    normalized.pop();
                }
                Some(Component::RootDir | Component::Prefix(_)) => {}
                _ => normalized.push(component.as_os_str()),
            },
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

fn normalize_for_comparison(path: &Path) -> PathBuf {
    if path.is_absolute() {
        normalize_path(path)
    } else if let Ok(cwd) = std::env::current_dir() {
        normalize_path(&cwd.join(path))
    } else {
        normalize_path(path)
    }
}

fn is_default_db_options(root_dir: &Path, options_file: Option<&Path>) -> Option<PathBuf> {
    let options_file = options_file?;
    let default_options_path = root_dir.join(DB_OPTIONS_FILE_NAME);
    (normalize_for_comparison(options_file) == normalize_for_comparison(&default_options_path))
        .then_some(default_options_path)
}

fn effective_sst_rebuild_options_file(
    root_dir: &Path,
    options_file: Option<&Path>,
) -> Option<PathBuf> {
    let options_file = options_file?;
    if is_default_db_options(root_dir, Some(options_file)).is_some() && !options_file.exists() {
        return None;
    }
    Some(options_file.to_path_buf())
}

fn sync_config_dir(root_dir: &Path) -> Result<(), Error> {
    #[cfg(unix)]
    fs::File::open(root_dir)
        .and_then(|dir| dir.sync_all())
        .map_err(|err| config_err(format!("Failed to sync {}: {err}", root_dir.display())))?;
    #[cfg(not(unix))]
    let _ = root_dir;
    Ok(())
}

fn warn_custom_legacy_db_options(root_dir: &Path, options_file: Option<&Path>) {
    let Some(options_file) = options_file else {
        return;
    };
    if is_default_db_options(root_dir, Some(options_file)).is_some() || !options_file.exists() {
        return;
    }
    if let Err(err) = RocksDB::validate_options_file(options_file, Schema::V1) {
        eprintln!(
            "WARNING: custom RocksDB options file {} will need to be updated before starting CKB:\n{err}",
            options_file.display()
        );
    }
}

fn config_err(msg: impl std::fmt::Display) -> Error {
    InternalErrorKind::Config.other(msg.to_string()).into()
}

fn refresh_default_db_options_if_needed(
    root_dir: &Path,
    options_file: Option<&Path>,
) -> Result<(), Error> {
    let Some(default_options_path) = is_default_db_options(root_dir, options_file) else {
        return Ok(());
    };

    if default_options_path.exists()
        && RocksDB::validate_options_file(&default_options_path, Schema::V1).is_ok()
    {
        sync_config_dir(root_dir)?;
        return Ok(());
    }

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|err| config_err(format!("Failed to generate backup timestamp: {err}")))?
        .as_nanos();
    let backup_path = root_dir.join(format!(
        "{DB_OPTIONS_FILE_NAME}.pre-sst-rebuild-{timestamp}"
    ));
    let staging_dir = root_dir.join(format!(".{DB_OPTIONS_FILE_NAME}.staging-{timestamp}"));
    fs::create_dir_all(&staging_dir)
        .map_err(|err| config_err(format!("Failed to create {}: {err}", staging_dir.display())))?;

    let context_for_db_options = TemplateContext::new("", vec![]);
    let staged_file = staging_dir.join(DB_OPTIONS_FILE_NAME);
    let stage_result = Resource::bundled_db_options()
        .export(&context_for_db_options, &staging_dir)
        .map_err(|err| {
            config_err(format!(
                "Failed to generate {}: {err}",
                default_options_path.display()
            ))
        })
        .and_then(|()| {
            // Open with write access so File::sync_all() (FlushFileBuffers) works on Windows too.
            OpenOptions::new()
                .write(true)
                .open(&staged_file)
                .and_then(|file| file.sync_all())
                .map_err(|err| {
                    config_err(format!("Failed to sync {}: {err}", staged_file.display()))
                })
        });
    if let Err(err) = stage_result {
        let _ = fs::remove_dir_all(&staging_dir);
        return Err(err);
    }

    let backed_up = default_options_path.exists();
    if backed_up && let Err(err) = fs::rename(&default_options_path, &backup_path) {
        let _ = fs::remove_dir_all(&staging_dir);
        return Err(config_err(format!(
            "Failed to back up {} to {}: {err}",
            default_options_path.display(),
            backup_path.display()
        )));
    }

    if let Err(err) = fs::rename(&staged_file, &default_options_path) {
        if backed_up {
            let _ = fs::rename(&backup_path, &default_options_path);
        }
        let _ = fs::remove_dir_all(&staging_dir);
        return Err(config_err(format!(
            "Failed to install {}: {err}",
            default_options_path.display()
        )));
    }
    let _ = fs::remove_dir_all(&staging_dir);
    sync_config_dir(root_dir)?;

    if backed_up {
        eprintln!(
            "Updated RocksDB options file {}; old file was backed up to {}",
            default_options_path.display(),
            backup_path.display(),
        );
    } else {
        eprintln!(
            "Generated RocksDB options file {}",
            default_options_path.display(),
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_path_preserves_leading_parent_dirs_and_root() {
        assert_eq!(
            normalize_path(Path::new("../../node/default.db-options")),
            PathBuf::from("../../node/default.db-options")
        );
        assert_eq!(
            normalize_path(Path::new("a/../b/./c")),
            PathBuf::from("b/c")
        );
        assert_eq!(normalize_path(Path::new("/../a")), PathBuf::from("/a"));
    }

    #[test]
    fn is_default_db_options_matches_relative_and_absolute_paths() {
        let cwd = std::env::current_dir().unwrap();
        let abs_default = cwd.join(DB_OPTIONS_FILE_NAME);
        assert_eq!(
            is_default_db_options(Path::new("."), Some(&abs_default)),
            Some(PathBuf::from(".").join(DB_OPTIONS_FILE_NAME))
        );
        let rel_root = Path::new("../../node");
        let rel_opts = Path::new("../../node/./default.db-options");
        assert_eq!(
            is_default_db_options(rel_root, Some(rel_opts)),
            Some(rel_root.join(DB_OPTIONS_FILE_NAME))
        );
    }

    #[test]
    fn refresh_default_db_options_preserves_v1_and_replaces_legacy() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let root_dir = tmp_dir.path();
        let default_options_path = root_dir.join(DB_OPTIONS_FILE_NAME);

        // Point options_file via an un-normalized `./default.db-options` path.
        let dot_relative_options = root_dir.join(".").join(DB_OPTIONS_FILE_NAME);

        // 1. Custom V1-compatible default.db-options should be preserved untouched.
        let custom_v1 = "[DBOptions]\nmax_background_jobs=12\n\n[CFOptions \"default\"]\nwrite_buffer_size=8388608\n";
        fs::write(&default_options_path, custom_v1).unwrap();
        refresh_default_db_options_if_needed(root_dir, Some(&dot_relative_options)).unwrap();
        assert_eq!(
            fs::read_to_string(&default_options_path).unwrap(),
            custom_v1
        );
        assert!(
            fs::read_dir(root_dir)
                .unwrap()
                .filter_map(Result::ok)
                .all(|e| !e
                    .file_name()
                    .to_string_lossy()
                    .starts_with("default.db-options.pre-sst-rebuild-"))
        );

        // 2. Legacy default.db-options with numeric CF sections should be backed up and replaced.
        let legacy_opts =
            "[DBOptions]\nmax_background_jobs=6\n\n[CFOptions \"1\"]\nwrite_buffer_size=8388608\n";
        fs::write(&default_options_path, legacy_opts).unwrap();
        refresh_default_db_options_if_needed(root_dir, Some(&dot_relative_options)).unwrap();
        RocksDB::validate_options_file(&default_options_path, Schema::V1).unwrap();
        assert!(
            fs::read_dir(root_dir)
                .unwrap()
                .filter_map(Result::ok)
                .any(|e| e
                    .file_name()
                    .to_string_lossy()
                    .starts_with("default.db-options.pre-sst-rebuild-"))
        );
    }

    #[test]
    fn effective_options_file_and_sync_config_dir_error_propagation() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let root_dir = tmp_dir.path();
        let default_opts = root_dir.join(DB_OPTIONS_FILE_NAME);
        let custom_opts = root_dir.join("custom.db-options");

        // Missing default.db-options is deferred to post-swap generation, while missing custom
        // options_file is passed through so preflight reports the error.
        assert_eq!(
            effective_sst_rebuild_options_file(root_dir, Some(&default_opts)),
            None
        );
        assert_eq!(
            effective_sst_rebuild_options_file(root_dir, Some(&custom_opts)),
            Some(custom_opts)
        );
        fs::write(&default_opts, "[DBOptions]\n").unwrap();
        assert_eq!(
            effective_sst_rebuild_options_file(root_dir, Some(&default_opts)),
            Some(default_opts)
        );

        #[cfg(unix)]
        {
            let missing_dir = root_dir.join("missing-dir");
            assert!(sync_config_dir(&missing_dir).is_err());
        }
    }
}
