use ckb_app_config::{ExitCode, ImportArgs};
use ckb_async_runtime::Handle;
use ckb_chain::chain::ChainService;
use ckb_instrument::Import;
use ckb_shared::SharedBuilder;

pub fn import(args: ImportArgs, async_handle: Handle) -> Result<(), ExitCode> {
    let (shared, mut pack) = SharedBuilder::new(&args.config.db, None, async_handle)
        .consensus(args.consensus)
        .build()
        .map_err(|err| {
            eprintln!("Import error: {:?}", err);
            ExitCode::Failure
        })?;

    let chain_service = ChainService::new(shared, pack.take_proposal_table());
    let chain_controller = chain_service.start::<&str>(Some("ImportChainService"));

    pack.take_tx_pool_builder();
    pack.take_relay_tx_receiver();

    Import::new(chain_controller, args.source)
        .execute()
        .map_err(|err| {
            eprintln!("Import error: {:?}", err);
            ExitCode::Failure
        })
}
