use ckb_app_config::{ExitCode, MigrateArgs};
use ckb_shared::shared::SharedBuilder;

pub fn migrate(args: MigrateArgs) -> Result<(), ExitCode> {
    let (_shared, _table) = SharedBuilder::with_db_config(&args.config.db)
        .build()
        .map_err(|err| {
            eprintln!("Run error: {:?}", err);
            ExitCode::Failure
        })?;
    Ok(())
}
