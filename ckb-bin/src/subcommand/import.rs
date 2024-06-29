use ckb_app_config::{ExitCode, ImportArgs};
use ckb_async_runtime::Handle;
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
    let (_shared, mut pack) = builder.build()?;

    let chain_controller = ckb_chain::start_chain_services(pack.take_chain_services_builder());

    // manual drop tx_pool_builder and relay_tx_receiver
    pack.take_tx_pool_builder();
    pack.take_relay_tx_receiver();

    Import::new(chain_controller, args.source)
        .execute()
        .map_err(|err| {
            eprintln!("Import error: {err:?}");
            ExitCode::Failure
        })
}
