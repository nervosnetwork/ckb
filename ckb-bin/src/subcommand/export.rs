use ckb_app_config::{ExitCode, ExportArgs};
use ckb_instrument::Export;
use ckb_shared::shared::SharedBuilder;

pub fn export(args: ExportArgs) -> Result<(), ExitCode> {
    let (shared, _) = SharedBuilder::with_db_config(&args.config.db)
        .consensus(args.consensus)
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
