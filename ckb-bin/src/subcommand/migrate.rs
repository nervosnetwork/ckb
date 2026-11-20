use ckb_app_config::{CKBAppConfig, ExitCode, MigrateArgs};
use ckb_migrate::migrate::Migrate;
use ckb_resource::{DB_OPTIONS_FILE_NAME, Resource, TemplateContext};
use is_terminal::IsTerminal;
use std::cmp::Ordering;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::helper::prompt;

pub fn migrate(args: MigrateArgs) -> Result<(), ExitCode> {
    let migrate = Migrate::new(&args.config.db.path, args.consensus.hardfork_switch);

    if args.sst_rebuild {
        if !args.force {
            if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
                let input = prompt(
                    "\
                    \n\
                    SST rebuild migration will create a new RocksDB database beside the current one,\n\
                    ingest generated SST files, validate the result, and then move the current DB to a backup path.\n\
                    \n\
                    CKB must be stopped while this runs. We strongly recommend backing up the data directory first.\n\
                    \nIf you want to rebuild the data, please input YES, otherwise, the current process will exit.\n\
                    > ",
                );
                if input.trim().to_lowercase() != "yes" {
                    eprintln!("Migration was declined since the user didn't confirm.");
                    return Err(ExitCode::Failure);
                }
            } else {
                eprintln!("Run error: use --force with --sst-rebuild without interactive prompt");
                return Err(ExitCode::Failure);
            }
        }

        migrate.sst_rebuild().map_err(|err| {
            eprintln!("Run error: {err:?}");
            ExitCode::Failure
        })?;
        refresh_default_db_options(&args.config)?;
        return Ok(());
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

fn refresh_default_db_options(config: &CKBAppConfig) -> Result<(), ExitCode> {
    let default_options_path = config.root_dir.join(DB_OPTIONS_FILE_NAME);
    if config.db.options_file.as_ref() != Some(&default_options_path) {
        return Ok(());
    }

    let backup_path = default_db_options_backup_path(&config.root_dir)?;
    let backed_up = default_options_path.exists();
    if backed_up {
        fs::rename(&default_options_path, &backup_path).map_err(|err| {
            eprintln!(
                "Failed to back up RocksDB options file {} to {}: {err}",
                default_options_path.display(),
                backup_path.display(),
            );
            ExitCode::Config
        })?;
    }

    let context_for_db_options = TemplateContext::new("", vec![]);
    if let Err(err) =
        Resource::bundled_db_options().export(&context_for_db_options, &config.root_dir)
    {
        if backed_up {
            let _ = fs::remove_file(&default_options_path);
            let _ = fs::rename(&backup_path, &default_options_path);
        }
        eprintln!(
            "Failed to generate RocksDB options file {}: {err}",
            default_options_path.display(),
        );
        return Err(ExitCode::Config);
    }

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

fn default_db_options_backup_path(root_dir: &std::path::Path) -> Result<PathBuf, ExitCode> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|err| {
            eprintln!("Failed to generate RocksDB options backup timestamp: {err}");
            ExitCode::Failure
        })?
        .as_nanos();
    Ok(root_dir.join(format!(
        "{DB_OPTIONS_FILE_NAME}.pre-sst-rebuild-{timestamp}"
    )))
}
