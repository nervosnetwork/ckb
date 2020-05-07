use ckb_app_config::{ExitCode, ProfArgs};
use ckb_chain::chain::ChainController;
use ckb_chain::chain::ChainService;
use ckb_logger::info;
use ckb_shared::shared::{Shared, SharedBuilder};
use ckb_store::ChainStore;
use std::sync::Arc;

pub fn profile(args: ProfArgs) -> Result<(), ExitCode> {
    let (shared, _table) = SharedBuilder::with_db_config(&args.config.db)
        .consensus(args.consensus.clone())
        .tx_pool_config(args.config.tx_pool)
        .build()
        .map_err(|err| {
            eprintln!("Prof error: {:?}", err);
            ExitCode::Failure
        })?;

    if !args.tmp_target.is_dir() {
        eprintln!(
            "Prof error: {:?}",
            "The specified path does not exist or not directory"
        );
        return Err(ExitCode::Failure);
    }
    let tmp_db_dir = tempfile::tempdir_in(args.tmp_target).map_err(|err| {
        eprintln!("Prof error: {:?}", err);
        ExitCode::Failure
    })?;
    {
        let mut tmp_db_config = args.config.db.clone();
        tmp_db_config.path = tmp_db_dir.path().to_path_buf();

        let (tmp_shared, table) = SharedBuilder::with_db_config(&tmp_db_config)
            .consensus(args.consensus)
            .tx_pool_config(args.config.tx_pool)
            .build()
            .map_err(|err| {
                eprintln!("Prof error: {:?}", err);
                ExitCode::Failure
            })?;

        let from = std::cmp::max(1, args.from);
        let to = std::cmp::min(shared.snapshot().tip_number(), args.to);
        let chain = ChainService::new(tmp_shared, table);
        let chain_controller = chain.start(Some("chain"));
        profile_block_process(
            shared.clone(),
            chain_controller.clone(),
            1,
            std::cmp::max(1, from.saturating_sub(1)),
        );
        info!("start profling, re-process blocks {}..{}:", from, to);
        let now = std::time::Instant::now();
        let tx_count = profile_block_process(shared, chain_controller, from, to);
        let duration = now.elapsed();
        info!(
            "end profling, duration {:?} txs {} tps {}",
            duration,
            tx_count,
            tx_count as u64 / duration.as_secs()
        );
    }

    tmp_db_dir.close().map_err(|err| {
        eprintln!("Prof error: {:?}", err);
        ExitCode::Failure
    })?;

    Ok(())
}

fn profile_block_process(
    shared: Shared,
    chain_controller: ChainController,
    from: u64,
    to: u64,
) -> usize {
    let mut tx_count = 0;
    let snapshot = shared.snapshot();
    for index in from..=to {
        let block = {
            let block_hash = snapshot.get_block_hash(index).unwrap();
            snapshot.get_block(&block_hash).unwrap()
        };
        tx_count += block.transactions().len().saturating_sub(1);
        chain_controller.process_block(Arc::new(block)).unwrap();
    }
    tx_count
}
