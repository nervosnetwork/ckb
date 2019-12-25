use ckb_app_config::{ExitCode, ImportArgs};
use ckb_chain::chain::ChainService;
use ckb_instrument::Import;
use ckb_shared::shared::SharedBuilder;

pub fn import(args: ImportArgs) -> Result<(), ExitCode> {
    let (shared, table) = SharedBuilder::with_db_config(&args.config.db)
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
