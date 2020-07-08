use ckb_app_config::{ExitCode, ExportArgs};
use ckb_async_runtime::Handle;
use ckb_instrument::Export;
use ckb_shared::shared::SharedBuilder;

pub fn export(args: ExportArgs, async_handle: Handle) -> Result<(), ExitCode> {
    let (shared, _) = SharedBuilder::new(&args.config.db, None, async_handle)
        .consensus(args.consensus)
        .build()
        .map_err(|err| {
            eprintln!("Export error: {:?}", err);
            ExitCode::Failure
        })?;
    Export::new(shared, args.target).execute().map_err(|err| {
        eprintln!("Export error: {:?}", err);
        ExitCode::Failure
    })
}
