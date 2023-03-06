use ckb_app_config::{ExitCode, MigrateArgs};
use ckb_launcher::migrate::Migrate;
use std::cmp::Ordering;

use crate::helper::prompt;

pub fn migrate(args: MigrateArgs) -> Result<(), ExitCode> {
    let migrate = Migrate::new(&args.config.db.path);

    {
        let read_only_db = migrate.open_read_only_db().map_err(|e| {
            eprintln!("migrate error {e}");
            ExitCode::Failure
        })?;

        if let Some(db) = read_only_db {
            let db_status = migrate.check(&db);
            if matches!(db_status, Ordering::Greater) {
                eprintln!(
                    "The database is created by a higher version CKB executable binary, \n\
                     so that the current CKB executable binary couldn't open this database.\n\
                     Please download the latest CKB executable binary."
                );
                return Err(ExitCode::Failure);
            }

            if args.check {
                if matches!(db_status, Ordering::Less) {
                    return Ok(());
                } else {
                    return Err(ExitCode::Cli);
                }
            }

            if matches!(db_status, Ordering::Equal) {
                return Ok(());
            }

            if migrate.require_expensive(&db) && !args.force {
                if atty::is(atty::Stream::Stdin) && atty::is(atty::Stream::Stdout) {
                    let input = prompt("\
                    \n\
                    Doing migration will take quite a long time before CKB could work again.\n\
                    Another choice is to delete all data, then synchronize them again.\n\
                    \n\
                    Once the migration started, the data will be no longer compatible with all older versions CKB,\n\
                    so we strongly recommended you to backup the old data before migrating.\n\
                    \nIf you want to migrate the data, please input YES, otherwise, the current process will exit.\n\
                    > ",
                    );
                    if input.trim().to_lowercase() != "yes" {
                        eprintln!("The migration was declined since the user didn't confirm.");
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
        eprintln!("migrate error {e}");
        ExitCode::Failure
    })?;

    if let Some(db) = bulk_load_db_db {
        migrate.migrate(db).map_err(|err| {
            eprintln!("Run error: {err:?}");
            ExitCode::Failure
        })?;
    }
    Ok(())
}
