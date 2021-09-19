use ckb_app_config::{ExitCode, ImportArgs};
use ckb_async_runtime::Handle;
use ckb_chain::chain::ChainService;
use ckb_instrument::Import;
use ckb_launcher::SharedBuilder;

pub fn import(args: ImportArgs, async_handle: Handle) -> Result<(), ExitCode> {
    let chain_cfg = args.config.chain.clone();
    let builder = SharedBuilder::new(
        &args.config.bin_name,
        args.config.root_dir.as_path(),
        &args.config.db,
        None,
        async_handle,
    )?;
    let (shared, mut pack) = builder.consensus(args.consensus).build()?;

    let chain_service =
        ChainService::new_with_config(shared, pack.take_proposal_table(), Some(chain_cfg));
    let chain_controller = chain_service.start::<&str>(Some("ImportChainService"));

    // manual drop tx_pool_builder and relay_tx_receiver
    pack.take_tx_pool_builder();
    pack.take_relay_tx_receiver();

    Import::new(chain_controller, args.source)
        .execute()
        .map_err(|err| {
            eprintln!("Import error: {:?}", err);
            ExitCode::Failure
        })
}
