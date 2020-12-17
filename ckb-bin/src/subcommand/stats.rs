use ckb_app_config::{ExitCode, StatsArgs};
use ckb_async_runtime::Handle;
use ckb_shared::shared::{Shared, SharedBuilder};
use ckb_store::ChainStore;
use ckb_types::core::BlockNumber;

pub fn stats(args: StatsArgs, async_handle: Handle) -> Result<(), ExitCode> {
    let stats = Statics::build(args, async_handle)?;
    stats.print_uncle_rate()?;
    Ok(())
}

struct Statics {
    shared: Shared,
    from: BlockNumber,
    to: BlockNumber,
}

impl Statics {
    pub fn build(args: StatsArgs, async_handle: Handle) -> Result<Self, ExitCode> {
        let (shared, _) = SharedBuilder::new(&args.config.db, None, async_handle)
            .consensus(args.consensus)
            .build()
            .map_err(|err| {
                eprintln!("Stats error: {:?}", err);
                ExitCode::Failure
            })?;

        let tip_number = shared.snapshot().tip_number();

        let from = args.from.unwrap_or(0);
        let to = args.to.unwrap_or_else(|| tip_number);

        if from >= to {
            return Err(ExitCode::Cli);
        }

        Ok(Statics { shared, from, to })
    }

    // exclusively below and above inclusively (from..to]
    pub fn print_uncle_rate(&self) -> Result<(), ExitCode> {
        let store = self.shared.store();
        let to_ext = store
            .get_block_hash(self.to)
            .and_then(|hash| store.get_block_ext(&hash))
            .ok_or_else(|| ExitCode::IO)?;
        let from_ext = store
            .get_block_hash(self.from)
            .and_then(|hash| store.get_block_ext(&hash))
            .ok_or_else(|| ExitCode::IO)?;

        let block_nums = self.to - self.from;
        let uncle_nums = to_ext.total_uncles_count - from_ext.total_uncles_count;

        println!(
            "uncle_rate: {}/{}({})",
            uncle_nums,
            block_nums,
            uncle_nums as f64 / block_nums as f64
        );
        Ok(())
    }
}
