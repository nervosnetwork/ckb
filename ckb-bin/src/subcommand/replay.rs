use ckb_app_config::{ExitCode, ReplayArgs};
use ckb_chain::{chain::ChainService, switch::Switch};
use ckb_chain_iter::ChainIterator;
use ckb_instrument::{ProgressBar, ProgressStyle};
use ckb_shared::shared::{Shared, SharedBuilder};
use ckb_store::ChainStore;
use std::sync::Arc;

pub fn replay(args: ReplayArgs) -> Result<(), ExitCode> {
    let (shared, _table) = SharedBuilder::with_db_config(&args.config.db)
        .consensus(args.consensus.clone())
        .tx_pool_config(args.config.tx_pool)
        .build()
        .map_err(|err| {
            eprintln!("replay error: {:?}", err);
            ExitCode::Failure
        })?;

    if !args.tmp_target.is_dir() {
        eprintln!(
            "replay error: {:?}",
            "The specified path does not exist or not directory"
        );
        return Err(ExitCode::Failure);
    }
    let tmp_db_dir = tempfile::tempdir_in(args.tmp_target).map_err(|err| {
        eprintln!("replay error: {:?}", err);
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
                eprintln!("replay error: {:?}", err);
                ExitCode::Failure
            })?;
        let chain = ChainService::new(tmp_shared, table);

        if let Some((from, to)) = args.profile {
            profile(shared, chain, from, to);
        } else if args.sanity_check {
            sanity_check(shared, chain, args.full_verfication);
        }
    }
    tmp_db_dir.close().map_err(|err| {
        eprintln!("replay error: {:?}", err);
        ExitCode::Failure
    })?;

    Ok(())
}

fn profile(shared: Shared, mut chain: ChainService, from: Option<u64>, to: Option<u64>) {
    let tip_number = shared.snapshot().tip_number();
    let from = from.map(|v| std::cmp::max(1, v)).unwrap_or(1);
    let to = to
        .map(|v| std::cmp::min(v, tip_number))
        .unwrap_or(tip_number);
    process_range_block(&shared, &mut chain, 1..from);
    println!("start profiling, re-process blocks {}..{}:", from, to);
    let now = std::time::Instant::now();
    let tx_count = process_range_block(&shared, &mut chain, from..=to);
    let duration = now.elapsed();
    println!(
        "end profiling, duration {:?} txs {} tps {}",
        duration,
        tx_count,
        tx_count as u64 / duration.as_secs()
    );
}

fn process_range_block(
    shared: &Shared,
    chain: &mut ChainService,
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
        chain.process_block(Arc::new(block), Switch::NONE).unwrap();
    }
    tx_count
}

fn sanity_check(shared: Shared, mut chain: ChainService, full_verfication: bool) {
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
    let switch = if full_verfication {
        Switch::NONE
    } else {
        Switch::DISABLE_ALL - Switch::DISABLE_NON_CONTEXTUAL
    };
    let mut cursor = shared.consensus().genesis_block().header();
    for block in chain_iter {
        let header = block.header();
        if let Err(e) = chain.process_block(Arc::new(block), switch) {
            eprintln!(
                "replay sanity-check error: {:?} at block({}-{})",
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
            "sanity-check break at block({}-{}), expect tip({}-{})",
            cursor.number(),
            cursor.hash(),
            tip_header.number(),
            tip_header.hash(),
        );
    } else {
        println!(
            "sanity-check pass, tip({}-{})",
            tip_header.number(),
            tip_header.hash()
        );
    }

    println!("replay finishing, please wait...");
}
