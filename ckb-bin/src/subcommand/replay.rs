use ckb_app_config::{ExitCode, ReplayArgs};
use ckb_async_runtime::Handle;
use ckb_chain::ChainController;
use ckb_chain_iter::ChainIterator;
use ckb_instrument::{ProgressBar, ProgressStyle};
use ckb_shared::{ChainServicesBuilder, Shared, SharedBuilder};
use ckb_store::ChainStore;
use ckb_verification_traits::Switch;
use std::sync::Arc;

const MIN_PROFILING_TIME: u64 = 5;

pub fn replay(args: ReplayArgs, async_handle: Handle) -> Result<(), ExitCode> {
    let shared_builder = SharedBuilder::new(
        &args.config.bin_name,
        args.config.root_dir.as_path(),
        &args.config.db,
        None,
        async_handle.clone(),
        args.consensus.clone(),
    )?;
    let (shared, _) = shared_builder
        .tx_pool_config(args.config.tx_pool.clone())
        .build()?;

    if !args.tmp_target.is_dir() {
        eprintln!(
            "Replay error: {:?}",
            "The specified path does not exist or not directory"
        );
        return Err(ExitCode::Failure);
    }
    let tmp_db_dir = tempfile::tempdir_in(args.tmp_target).map_err(|err| {
        eprintln!("Replay error: {err:?}");
        ExitCode::Failure
    })?;
    {
        let mut tmp_db_config = args.config.db.clone();
        tmp_db_config.path = tmp_db_dir.path().to_path_buf();

        let shared_builder = SharedBuilder::new(
            &args.config.bin_name,
            args.config.root_dir.as_path(),
            &tmp_db_config,
            None,
            async_handle,
            args.consensus,
        )?;
        let (_tmp_shared, mut pack) = shared_builder.tx_pool_config(args.config.tx_pool).build()?;
        let chain_service_builder: ChainServicesBuilder = pack.take_chain_services_builder();
        let chain_controller = ckb_chain::start_chain_services(chain_service_builder);

        if let Some((from, to)) = args.profile {
            profile(shared, chain_controller, from, to);
        } else if args.sanity_check {
            sanity_check(shared, chain_controller, args.full_verification);
        }
    }
    tmp_db_dir.close().map_err(|err| {
        eprintln!("Replay error: {err:?}");
        ExitCode::Failure
    })?;

    Ok(())
}

fn profile(shared: Shared, chain_controller: ChainController, from: Option<u64>, to: Option<u64>) {
    let tip_number = shared.snapshot().tip_number();
    let from = from.map(|v| std::cmp::max(1, v)).unwrap_or(1);
    let to = to
        .map(|v| std::cmp::min(v, tip_number))
        .unwrap_or(tip_number);
    process_range_block(&shared, chain_controller.clone(), 1..from);
    println!("Start profiling, re-process blocks {from}..{to}:");
    let now = std::time::Instant::now();
    let tx_count = process_range_block(&shared, chain_controller, from..=to);
    let duration = std::time::Instant::now().saturating_duration_since(now);
    if duration.as_secs() >= MIN_PROFILING_TIME {
        println!(
            "\n----------------------------\nEnd profiling, duration:{:?}, txs:{}, tps:{}\n----------------------------",
            duration,
            tx_count,
            tx_count as u64 / duration.as_secs()
        );
    } else {
        println!(
            concat!(
                "----------------------------\n",
                r#"Profiling with too short time({:?}) is inaccurate and referential; it's recommended to modify"#,
                "\n",
                r#"parameters(--from, --to) to increase block range, to make profiling time is greater than "#,
                "{} seconds\n----------------------------",
            ),
            duration, MIN_PROFILING_TIME
        );
    }
}

fn process_range_block(
    shared: &Shared,
    chain_controller: ChainController,
    range: impl Iterator<Item = u64>,
) -> usize {
    let mut tx_count = 0;
    let snapshot = shared.snapshot();
    for index in range {
        let block = snapshot
            .get_block_hash(index)
            .and_then(|hash| snapshot.get_block(&hash))
            .expect("read block from store");
        tx_count += block.transactions().len().saturating_sub(1);
        chain_controller
            .blocking_process_block_with_switch(Arc::new(block), Switch::NONE)
            .unwrap();
    }
    tx_count
}

fn sanity_check(shared: Shared, chain_controller: ChainController, full_verification: bool) {
    let tip_header = shared.snapshot().tip_header().clone();
    let chain_iter = ChainIterator::new(shared.store());
    let pb = ProgressBar::new(chain_iter.len());
    pb.set_style(
        ProgressStyle::default_bar()
            .template(
                "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta})",
            )
            .progress_chars("#>-"),
    );
    let switch = if full_verification {
        Switch::NONE
    } else {
        Switch::DISABLE_ALL - Switch::DISABLE_NON_CONTEXTUAL
    };
    let mut cursor = shared.consensus().genesis_block().header();
    for block in chain_iter {
        let header = block.header();
        if let Err(e) = chain_controller.blocking_process_block_with_switch(Arc::new(block), switch)
        {
            eprintln!(
                "Replay sanity-check error: {:?} at block({}-{})",
                e,
                header.number(),
                header.hash(),
            );
            pb.finish_with_message("replay finish");
            return;
        } else {
            pb.inc(1);
            cursor = header;
        }
    }
    pb.finish_with_message("finish");

    if cursor != tip_header {
        eprintln!(
            "Sanity-check break at block({}-{}); expect tip({}-{})",
            cursor.number(),
            cursor.hash(),
            tip_header.number(),
            tip_header.hash(),
        );
    } else {
        println!(
            "Sanity-check pass, tip({}-{})",
            tip_header.number(),
            tip_header.hash()
        );
    }

    println!("Finishing replay; please wait...");
}
