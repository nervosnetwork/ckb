use ckb_app_config::{ExitCode, ProfArgs};
use ckb_chain::chain::ChainController;
use ckb_chain::chain::ChainService;
use ckb_db::{DBConfig, RocksDB};
use ckb_logger::info;
use ckb_notify::NotifyService;
use ckb_shared::shared::{Shared, SharedBuilder};
use ckb_store::ChainStore;
use ckb_traits::ChainProvider;
use std::sync::Arc;

pub fn profile(args: ProfArgs) -> Result<(), ExitCode> {
    let shared = SharedBuilder::<RocksDB>::default()
        .consensus(args.consensus.clone())
        .db(&args.config.db)
        .tx_pool_config(args.config.tx_pool)
        .script_config(args.config.script)
        .build()
        .map_err(|err| {
            eprintln!("Prof error: {:?}", err);
            ExitCode::Failure
        })?;

    let tmp_dir = tempfile::Builder::new().tempdir().unwrap();
    let tmp_shared = SharedBuilder::<RocksDB>::default()
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
    let to = std::cmp::min(shared.lock_chain_state().tip_number(), args.to);
    let notify = NotifyService::default().start::<&str>(Some("notify"));
    let chain = ChainService::new(tmp_shared, notify);
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
    Ok(())
}

fn profile_block_process<CS: ChainStore + 'static>(
    shared: Shared<CS>,
    chain_controller: ChainController,
    from: u64,
    to: u64,
) -> usize {
    let mut tx_count = 0;
    for index in from..=to {
        let block = {
            let block_hash = shared.store().get_block_hash(index).unwrap();
            shared.store().get_block(&block_hash).unwrap()
        };
        tx_count += block.transactions().len().saturating_sub(1);
        chain_controller
            .process_block(Arc::new(block), true)
            .unwrap();
    }
    tx_count
}
