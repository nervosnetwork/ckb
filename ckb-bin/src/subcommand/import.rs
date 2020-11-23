use ckb_app_config::{ExitCode, ImportArgs};
use ckb_async_runtime::Handle;
use ckb_chain::chain::ChainService;
use ckb_instrument::Import;
use ckb_shared::shared::SharedBuilder;

pub fn import(args: ImportArgs, async_handle: Handle) -> Result<(), ExitCode> {
    let (shared, table) = SharedBuilder::new(&args.config.db, async_handle)
        .consensus(args.consensus)
        .build()
        .map_err(|err| {
            eprintln!("Import error: {:?}", err);
            ExitCode::Failure
        })?;

    let chain_service = ChainService::new(shared, table);
    let chain_controller = chain_service.start::<&str>(Some("ImportChainService"));

    Import::new(chain_controller, args.source)
        .execute()
        .map_err(|err| {
            eprintln!("Import error: {:?}", err);
            ExitCode::Failure
        })
}
