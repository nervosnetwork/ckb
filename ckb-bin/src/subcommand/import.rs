use ckb_app_config::{ExitCode, ImportArgs};
use ckb_async_runtime::Handle;
use ckb_chain::ChainServiceScope;
use ckb_instrument::Import;
use ckb_shared::SharedBuilder;

pub fn import(args: ImportArgs, async_handle: Handle) -> Result<(), ExitCode> {
    let builder = SharedBuilder::new(
        &args.config.bin_name,
        args.config.root_dir.as_path(),
        &args.config.db,
        None,
        async_handle,
        args.consensus,
    )?;
    let (shared, pack) = builder.build()?;

    let chain_scope = ChainServiceScope::new(pack.into_chain_services_builder());
    let chain_controller = chain_scope.chain_controller().clone();

    Import::new(
        chain_controller,
        shared,
        args.source,
        args.switch,
        args.num_threads,
    )
    .execute()
    .map_err(|err| {
        eprintln!("Import error: {err:?}");
        ExitCode::Failure
    })
}
