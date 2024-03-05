use ckb_app_config::{ExitCode, TruncateBlockArgs};
use ckb_async_runtime::Handle;
use ckb_chain::chain::ChainService;
use ckb_proposal_table::ProposalTable;
use ckb_shared::{Shared, SharedBuilder};
use ckb_store::ChainStore;
use ckb_types::core::BlockNumber;
use std::sync::Arc;

struct TruncateBlock {
    shared_builder: SharedBuilder,
    proposal_table: ProposalTable,
    from: BlockNumber,
}

impl TruncateBlock {
    pub fn build(args: TruncateBlockArgs, async_handle: Handle) -> Result<Self, ExitCode> {
        let shared_builder = SharedBuilder::new(
            &args.config.bin_name,
            args.config.root_dir.as_path(),
            &args.config.db,
            None,
            async_handle,
            args.consensus,
        )?;

        let from = args.from.unwrap_or(0);
        let proposal_table = pack.take_proposal_table();

        Ok(TruncateBlock {
            shared_builder,
            proposal_table,
            from,
        })
    }

    pub fn truncate_blocks(&mut self) -> Result<(), ExitCode> {
        let (shared, mut pack) = self.shared_builder.build()?;
        let proposal_table = pack.take_proposal_table();
        let snapshot = Arc::clone(&shared.snapshot());

        let target_tip_hash = &snapshot.get_block_hash(self.from).ok_or(ExitCode::IO)?;

        ChainService::new(shared.clone(), proposal_table)
            .truncate(target_tip_hash)
            .map_err(|err| {
                eprintln!("truncate block error: {:?}", err);
                ExitCode::Failure
            })
    }
}
