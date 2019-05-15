use ckb_app_config::{ExitCode, ExportArgs};
use ckb_db::RocksDB;
use ckb_instrument::Export;
use ckb_shared::shared::SharedBuilder;

pub fn export(args: ExportArgs) -> Result<(), ExitCode> {
    let shared = SharedBuilder::<RocksDB>::default()
        .consensus(args.consensus)
        .db(&args.config.db)
        .build()
        .map_err(|err| {
            eprintln!("Export error: {:?}", err);
            ExitCode::Failure
        })?;
    Export::new(shared, args.format, args.target)
        .execute()
        .map_err(|err| {
            eprintln!("Export error: {:?}", err);
            ExitCode::Failure
        })
}
