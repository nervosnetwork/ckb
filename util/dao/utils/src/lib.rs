use ckb_core::cell::{ResolvedCell, ResolvedTransaction};
use ckb_core::script::DAO_CODE_HASH;
use ckb_core::{Bytes, Capacity};
use ckb_store::{ChainStore, LazyLoadCellOutput};
use dao::calculate_maximum_withdraw;
use numext_fixed_hash::H256;
use std::sync::Arc;

// With DAO in consideration, transaction fee calculation is getting more
// complicated, this utility provides a quicker way to calculate fees.
// Notice this is just a tool focusing on calculating transactino fees, it
// would just emit None silently for cases like missing/invalid block hash.
// Those validation work is left to other verifiers to check.
// TODO: revisit it here to see if we need to return correct error once we
// manage to revise error handling in verification package
pub fn calculate_transaction_fee<CS: ChainStore>(
    store: Arc<CS>,
    rtx: &ResolvedTransaction,
) -> Option<Capacity> {
    rtx.transaction
        .inputs()
        .iter()
        .zip(rtx.resolved_inputs.iter())
        .try_fold(
            Capacity::zero(),
            |input_capacities, (input, resolved_input)| {
                let capacity = match &resolved_input.cell {
                    ResolvedCell::IssuingDaoInput => Some(Capacity::zero()),
                    ResolvedCell::Null => None,
                    ResolvedCell::Cell(cell_meta) => {
                        let output = store.lazy_load_cell_output(&cell_meta);
                        if output.lock.code_hash == DAO_CODE_HASH {
                            let deposit_ext = input
                                .previous_output
                                .block_hash
                                .as_ref()
                                .and_then(|block_hash| store.get_block_ext(&block_hash));
                            // The first argument of DAO input should contain withdraw
                            // block hash.
                            let withdraw_ext = input.args.get(0).and_then(|arg: &Bytes| {
                                H256::from_slice(&arg)
                                    .ok()
                                    .as_ref()
                                    .and_then(|block_hash| store.get_block_ext(block_hash))
                            });
                            match (deposit_ext, withdraw_ext) {
                                (Some(deposit_ext), Some(withdraw_ext)) => {
                                    calculate_maximum_withdraw(&output, &deposit_ext, &withdraw_ext)
                                        .ok()
                                }
                                _ => None,
                            }
                        } else {
                            Some(output.capacity)
                        }
                    }
                };
                capacity.and_then(|c| c.safe_add(input_capacities).ok())
            },
        )
        .and_then(|x| {
            rtx.transaction
                .outputs_capacity()
                .and_then(|y| {
                    if x > y {
                        x.safe_sub(y)
                    } else {
                        Ok(Capacity::zero())
                    }
                })
                .ok()
        })
}
