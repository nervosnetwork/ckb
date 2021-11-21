use ckb_app_config::{ExitCode, StatsArgs};
use ckb_async_runtime::Handle;
use ckb_launcher::SharedBuilder;
use ckb_shared::Shared;
use ckb_store::ChainStore;
use ckb_types::{
    core::{BlockNumber, ScriptHashType},
    packed::CellbaseWitness,
    prelude::*,
};
use std::cmp::max;
use std::collections::HashMap;
use std::convert::TryFrom;

pub fn stats(args: StatsArgs, async_handle: Handle) -> Result<(), ExitCode> {
    let stats = Statics::build(args, async_handle)?;
    stats.print_uncle_rate()?;
    stats.print_miner_statics()?;
    Ok(())
}

struct Statics {
    shared: Shared,
    from: BlockNumber,
    to: BlockNumber,
}

impl Statics {
    pub fn build(args: StatsArgs, async_handle: Handle) -> Result<Self, ExitCode> {
        let shared_builder = SharedBuilder::new(
            &args.config.bin_name,
            args.config.root_dir.as_path(),
            &args.config.db,
            None,
            async_handle,
        )?;
        let (shared, _) = shared_builder.consensus(args.consensus).build()?;

        let tip_number = shared.snapshot().tip_number();

        let from = args.from.unwrap_or(0);
        let to = args.to.unwrap_or(tip_number);

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
            .ok_or(ExitCode::IO)?;
        let from_ext = store
            .get_block_hash(self.from)
            .and_then(|hash| store.get_block_ext(&hash))
            .ok_or(ExitCode::IO)?;

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

    fn print_miner_statics(&self) -> Result<(), ExitCode> {
        let store = self.shared.store();
        let mut by_miner_script = HashMap::new();
        let mut by_miner_message = HashMap::new();
        // count maximum 1000 blocks
        let from = max(self.to.saturating_sub(999), self.from);
        for i in from..=self.to {
            let cellbase = store
                .get_block_hash(i)
                .and_then(|hash| store.get_cellbase(&hash))
                .ok_or(ExitCode::IO)?;
            let cellbase_witness = cellbase
                .witnesses()
                .get(0)
                .and_then(|witness| CellbaseWitness::from_slice(&witness.raw_data()).ok())
                .expect("cellbase witness should be ok");
            by_miner_script
                .entry(cellbase_witness.lock())
                .and_modify(|e| *e += 1)
                .or_insert(1);
            by_miner_message
                .entry(cellbase_witness.message().raw_data())
                .and_modify(|e| *e += 1)
                .or_insert(1);
        }
        let total = self.to - from;

        let mut by_miner_script_vec: Vec<_> = by_miner_script.into_iter().collect();
        by_miner_script_vec.sort_by_key(|(_, v)| *v);
        println!("by_miner_script:");
        println!(
            "{0: <10} | {1: <5} | {2: <40} | {3: <64} | {4: <9} ",
            "percentage", "total", "args", "code_hash", "hash_type"
        );
        for (script, count) in by_miner_script_vec.iter().rev() {
            println!(
                "{0: <10} | {1: <5} | {2: <40x} | {3: <64x} | {4: <9?}",
                format!("{:.1}%", 100.0 * (*count as f64) / (total as f64)),
                count,
                script.args().raw_data(),
                script.code_hash(),
                ScriptHashType::try_from(script.hash_type()).expect("checked script hash type"),
            );
        }

        let mut by_miner_message_vec: Vec<_> = by_miner_message.into_iter().collect();
        by_miner_message_vec.sort_by_key(|(_, v)| *v);
        println!("by_miner_message:");
        println!(
            "{0: <10} | {1: <5} | {2: <50}",
            "percentage", "total", "messages"
        );
        for (message, count) in by_miner_message_vec.iter().rev() {
            println!(
                "{0: <10} | {1: <5} | {2: <50?}",
                format!("{:.1}%", 100.0 * (*count as f64) / (total as f64)),
                count,
                message,
            );
        }
        Ok(())
    }
}
