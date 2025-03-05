use ckb_app_config::{ExitCode, MigrateArgs};
use ckb_migrate::migrate::Migrate;
use is_terminal::IsTerminal;
use std::cmp::Ordering;

use crate::helper::prompt;

pub fn migrate(args: MigrateArgs) -> Result<(), ExitCode> {
    let migrate = Migrate::new(&args.config.db.path, args.consensus.hardfork_switch);

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
