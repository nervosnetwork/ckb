use ckb_app_config::{ExitCode, MigrateArgs};
use ckb_shared::shared::SharedBuilder;

pub fn migrate(args: MigrateArgs) -> Result<(), ExitCode> {
    let builder = SharedBuilder::with_db_config(&args.config.db);

    if args.check {
        if builder.migration_check() {
            return Ok(());
        } else {
            return Err(ExitCode::Cli);
        }
    }

    let (_shared, _table) = builder.build().map_err(|err| {
        eprintln!("Run error: {:?}", err);
        ExitCode::Failure
    })?;
    Ok(())
}
