use ckb_app_config::{ExitCode, ExportArgs};
use ckb_async_runtime::Handle;
use ckb_instrument::Export;
use ckb_shared::SharedBuilder;

pub fn export(args: ExportArgs, async_handle: Handle) -> Result<(), ExitCode> {
    let builder = SharedBuilder::new(
        &args.config.bin_name,
        args.config.root_dir.as_path(),
        &args.config.db,
        None,
        async_handle,
        args.consensus,
    )?;
    let (shared, _) = builder.build()?;
    Export::new(shared, args.target).execute().map_err(|err| {
        eprintln!("Export error: {err:?}");
        ExitCode::Failure
    })
}
