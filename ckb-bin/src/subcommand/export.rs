use ckb_app_config::{ExitCode, ExportArgs};
use ckb_async_runtime::Handle;
use ckb_instrument::Export;
use ckb_jsonrpc_types::Either;
use ckb_shared::SharedBuilder;
use ckb_types::{H256, core::BlockNumber};

fn parse_from_to_arg(arg: Either<u64, String>) -> Result<Either<BlockNumber, H256>, ExitCode> {
    match arg {
        Either::Left(num) => Ok(Either::Left(BlockNumber::from(num))),
        Either::Right(hash_str) => H256::from_trimmed_str(&hash_str)
            .map(Either::Right)
            .map_err(|_| {
                eprintln!("Invalid block hash provided: {}", hash_str);
                ExitCode::Failure
            }),
    }
}

pub fn export(args: ExportArgs, async_handle: Handle) -> Result<(), ExitCode> {
    let builder = SharedBuilder::new(
        &args.config.bin_name,
        args.config.root_dir.as_path(),
        &args.config.db,
        None,
        async_handle,
        args.consensus,
    )?;
    let (shared, _) = builder.build()?;

    let from = args.from.map(parse_from_to_arg).transpose()?;

    let to = args.to.map(parse_from_to_arg).transpose()?;

    Export::new(shared, args.target, from, to)
        .execute()
        .map_err(|err| {
            eprintln!("Export error: {err:?}");
            ExitCode::Failure
        })
}
