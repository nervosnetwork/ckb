use ckb_app_config::{ExitCode, ImportArgs};
use ckb_chain::chain::ChainBuilder;
use ckb_db::{CacheDB, RocksDB};
use ckb_instrument::Import;
use ckb_notify::NotifyService;
use ckb_shared::shared::SharedBuilder;

pub fn import(args: ImportArgs) -> Result<(), ExitCode> {
    let shared = SharedBuilder::<CacheDB<RocksDB>>::default()
        .consensus(args.consensus)
        .db(&args.config.db)
        .build();

    let notify = NotifyService::default().start::<&str>(None);
    let chain_service = ChainBuilder::new(shared.clone(), notify).build();
    let chain_controller = chain_service.start::<&str>(Some("ImportChainService"));

    Import::new(chain_controller, args.format, args.source)
        .execute()
        .map_err(|err| {
            eprintln!("Import error: {:?}", err);
            ExitCode::Failure
        })
}
