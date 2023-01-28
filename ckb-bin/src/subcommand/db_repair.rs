use ckb_app_config::{ExitCode, RepairArgs};
use ckb_db::RocksDB;

pub fn db_repair(args: RepairArgs) -> Result<(), ExitCode> {
    RocksDB::repair(&args.config.db.path).map_err(|err| {
        eprintln!("repair error: {err:?}");
        ExitCode::Failure
    })?;

    Ok(())
}
