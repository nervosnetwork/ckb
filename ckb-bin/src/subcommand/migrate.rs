use ckb_app_config::{ExitCode, MigrateArgs};
use ckb_async_runtime::Handle;
use ckb_shared::shared::SharedBuilder;

use crate::helper::prompt;

pub fn migrate(args: MigrateArgs, async_handle: Handle) -> Result<(), ExitCode> {
    let builder = SharedBuilder::new(&args.config.db, None, async_handle);

    if args.check {
        if builder.migration_check() {
            return Ok(());
        } else {
            return Err(ExitCode::Cli);
        }
    }

    if !builder.migration_check() {
        return Ok(());
    }

    if builder.require_expensive_migrations() && !args.force {
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

    let (_shared, _table) = builder.consensus(args.consensus).build().map_err(|err| {
        eprintln!("Run error: {:?}", err);
        ExitCode::Failure
    })?;
    Ok(())
}
