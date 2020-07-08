use ckb_app_config::{ExitCode, MigrateArgs};
use ckb_async_runtime::Handle;
use ckb_shared::shared::SharedBuilder;

pub fn migrate(args: MigrateArgs, async_handle: Handle) -> Result<(), ExitCode> {
    let builder = SharedBuilder::new(&args.config.db, None, async_handle);

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
