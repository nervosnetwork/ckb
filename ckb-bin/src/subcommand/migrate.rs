use ckb_app_config::{ExitCode, MigrateArgs};
use ckb_launcher::DatabaseMigration;

use crate::helper::prompt;

pub fn migrate(args: MigrateArgs) -> Result<(), ExitCode> {
    let migration = DatabaseMigration::new(&args.config.db.path);

    if args.check {
        if migration.migration_check() {
            return Ok(());
        } else {
            return Err(ExitCode::Cli);
        }
    }

    if !migration.migration_check() {
        return Ok(());
    }

    if migration.require_expensive_migrations() && !args.force {
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

    migration.migrate().map_err(|err| {
        eprintln!("Run error: {:?}", err);
        ExitCode::Failure
    })?;

    Ok(())
}
