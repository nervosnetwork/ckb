use ckb_app_config::{ExitCode, ProfArgs};
use ckb_chain::chain::ChainBuilder;
use ckb_db::{CacheDB, DBConfig, RocksDB};
use ckb_notify::NotifyService;
use ckb_shared::shared::{Shared, SharedBuilder};
use ckb_store::ChainStore;
use log::info;
use std::sync::Arc;

pub fn profile(args: ProfArgs) -> Result<(), ExitCode> {
    let shared = SharedBuilder::<CacheDB<RocksDB>>::default()
        .consensus(args.consensus.clone())
        .db(&args.config.db)
        .tx_pool_config(args.config.tx_pool.clone())
        .build()
        .map_err(|err| {
            eprintln!("Prof error: {:?}", err);
            ExitCode::Failure
        })?;

    let tmp_dir = tempfile::Builder::new().tempdir().unwrap();
    let tmp_shared = SharedBuilder::<CacheDB<RocksDB>>::default()
        .consensus(args.consensus)
        .db(&DBConfig {
            path: tmp_dir.as_ref().to_path_buf(),
            options: None,
        })
        .tx_pool_config(args.config.tx_pool)
        .build()
        .map_err(|err| {
            eprintln!("Prof error: {:?}", err);
            ExitCode::Failure
        })?;

    let from = std::cmp::max(1, args.from);
    let to = std::cmp::min(shared.chain_state().lock().tip_number(), args.to);
    info!("start profling, re-process blocks {}..{}:", from, to);
    let now = std::time::Instant::now();
    let tx_count = profile_block_process(shared, tmp_shared, from, to);
    let duration = now.elapsed();
    info!(
        "end profling, duration {:?} txs {} tps {}",
        duration,
        tx_count,
        tx_count as u64 / duration.as_secs()
    );
    Ok(())
}

fn profile_block_process<CS: ChainStore + 'static>(
    shared: Shared<CS>,
    tmp_shared: Shared<CS>,
    from: u64,
    to: u64,
) -> usize {
    let notify = NotifyService::default().start::<&str>(Some("notify"));
    let chain = ChainBuilder::new(tmp_shared, notify).build();
    let chain_controller = chain.start(Some("chain"));
    let mut tx_count = 0;
    for index in from..=to {
        let block = {
            let block_hash = shared.store().get_block_hash(index).unwrap();
            shared.store().get_block(&block_hash).unwrap()
        };
        tx_count += block.transactions().len();
        chain_controller.process_block(Arc::new(block)).unwrap();
    }
    tx_count
}
