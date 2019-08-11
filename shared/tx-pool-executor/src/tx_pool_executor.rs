use ckb_error::Error;
use ckb_shared::shared::Shared;
use ckb_shared::Snapshot;
use ckb_store::ChainStore;
use ckb_traits::chain_provider::ChainProvider;
use ckb_traits::BlockMedianTimeContext;
use ckb_types::{
    core::{BlockNumber, Cycle, TransactionView},
    packed::Byte32,
};
use ckb_verification::TransactionVerifier;
use std::collections::HashMap;

struct StoreBlockMedianTimeContext<'a, CS> {
    store: &'a CS,
    median_time_block_count: u64,
}

impl<'a, CS: ChainStore<'a>> BlockMedianTimeContext for StoreBlockMedianTimeContext<'a, CS> {
    fn median_block_count(&self) -> u64 {
        self.median_time_block_count
    }

    fn timestamp_and_parent(&self, block_hash: &Byte32) -> (u64, BlockNumber, Byte32) {
        let header = self
            .store
            .get_block_header(block_hash)
            .expect("[StoreBlockMedianTimeContext] blocks used for median time exist");
        (
            header.timestamp(),
            header.number(),
            header.data().raw().parent_hash(),
        )
    }
}

/// TxPoolExecutor
/// execute txs in parallel then add them to tx_pool
pub struct TxPoolExecutor {
    shared: Shared,
}

impl TxPoolExecutor {
    pub fn new(shared: Shared) -> TxPoolExecutor {
        TxPoolExecutor { shared }
    }

    pub fn verify_and_add_tx_to_pool(&self, tx: TransactionView) -> Result<Cycle, Error> {
        self.verify_and_add_txs_to_pool(vec![tx])
            .map(|cycles_vec| *cycles_vec.get(0).expect("tx verified cycles"))
    }

    pub fn verify_and_add_txs_to_pool(
        &self,
        txs: Vec<TransactionView>,
    ) -> Result<Vec<Cycle>, Error> {
        if txs.is_empty() {
            return Ok(Vec::new());
        }
        // resolve txs
        // early release the chain_state lock because tx verification is slow
        let snapshot: &Snapshot = &self.shared.snapshot();
        let (
            resolved_txs,
            cached_txs,
            mut unresolvable_txs,
            consensus,
            parent_number,
            epoch_number,
            parent_hash,
        ) = {
            let tx_pool = self.shared.try_lock_tx_pool();
            let txs_verify_cache = self.shared.lock_txs_verify_cache();
            let consensus = self.shared.consensus();
            let parent_number = snapshot.tip_number();
            let parent_hash = snapshot.tip_hash().to_owned();
            let epoch_number = snapshot.epoch_ext().number();
            let mut resolved_txs = Vec::with_capacity(txs.len());
            let mut unresolvable_txs = Vec::with_capacity(txs.len());
            let mut cached_txs = Vec::with_capacity(txs.len());
            for tx in &txs {
                if let Some(cycles) = txs_verify_cache.get(&tx.hash()) {
                    cached_txs.push((tx.hash(), Ok(*cycles)));
                } else {
                    match tx_pool.resolve_tx_from_pending_and_proposed(tx) {
                        Ok(resolved_tx) => resolved_txs.push((tx.hash(), resolved_tx)),
                        Err(err) => unresolvable_txs
                            .push((tx.hash(), err)),
                    }
                }
            }
            (
                resolved_txs,
                cached_txs,
                unresolvable_txs,
                consensus,
                parent_number,
                epoch_number,
                parent_hash,
            )
        };

        // immediately return if resolved_txs is empty
        if resolved_txs.is_empty() && cached_txs.is_empty() {
            let (_, err) = unresolvable_txs.remove(0);
            return Err(err);
        }

        let max_block_cycles = consensus.max_block_cycles();
        let block_median_time_context = StoreBlockMedianTimeContext {
            store: snapshot,
            median_time_block_count: consensus.median_time_block_count() as u64,
        };

        // parallel verify txs
        let cycles_vec = resolved_txs
            .iter()
            .map(|(tx_hash, tx)| {
                let verified_result = TransactionVerifier::new(
                    &tx,
                    &block_median_time_context,
                    parent_number + 1,
                    epoch_number,
                    parent_hash.clone(),
                    &consensus,
                    self.shared.script_config(),
                    snapshot,
                )
                .verify(max_block_cycles)
                .map(|cycles| (tx, cycles));
                (tx_hash.to_owned(), verified_result)
            })
            .collect::<Vec<_>>();

        // Add verified txs to pool
        // must lock chain_state before txs_verify_cache to avoid dead lock.
        let mut tx_pool = self.shared.try_lock_tx_pool();
        // write cache
        let cycles_vec = {
            let mut txs_verify_cache = self.shared.lock_txs_verify_cache();
            cycles_vec
                .into_iter()
                .map(|(i, result)| {
                    let result = match result {
                        Ok((rtx, cycles)) => {
                            txs_verify_cache.insert(rtx.transaction.hash(), cycles);
                            Ok(cycles)
                        }
                        Err(err) => Err(err),
                    };
                    (i, result)
                })
                .collect::<Vec<(Byte32, Result<Cycle, _>)>>()
        };

        // join all txs
        let cycles_vec = {
            let mut cycles_vec = cycles_vec
                .into_iter()
                .chain(cached_txs)
                .chain(unresolvable_txs.into_iter().map(|(tx, err)| (tx, Err(err))))
                .collect::<HashMap<Byte32, Result<Cycle, Error>>>();
            txs.iter()
                .map(|tx| {
                    cycles_vec
                        .remove(&tx.hash())
                        .expect("verified tx should exists")
                        .map(|cycles| (cycles, tx.to_owned()))
                })
                .collect::<Vec<Result<(Cycle, TransactionView), Error>>>()
        };
        cycles_vec
            .into_iter()
            .map(|result| match result {
                Ok((cycles, tx)) => tx_pool.add_tx_to_pool(tx, cycles),
                Err(err) => Err(err),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ckb_chain::chain::ChainService;
    use ckb_chain_spec::consensus::Consensus;
    use ckb_core::error::OutPointError;
    use ckb_error::assert_error_eq;
    use ckb_notify::NotifyService;
    use ckb_shared::shared::{Shared, SharedBuilder};
    use ckb_test_chain_utils::always_success_cell;
    use ckb_traits::ChainProvider;
    use ckb_types::{
        bytes::Bytes,
        core::{
            capacity_bytes, cell::UnresolvableError, BlockBuilder, Capacity, DepType,
            TransactionBuilder,
        },
        packed::{CellDep, CellInput, CellOutput, OutPoint},
        prelude::*,
        U256,
    };
    use ckb_verification::TransactionError;
    use faketime::{self, unix_time_as_millis};
    use std::sync::Arc;

    fn setup(height: u64) -> (Shared, OutPoint) {
        let (always_success_cell, always_success_cell_data, always_success_script) =
            always_success_cell();
        let always_success_tx = TransactionBuilder::default()
            .witness(always_success_script.clone().into_witness())
            .input(CellInput::new(OutPoint::null(), 0))
            .output(always_success_cell.clone())
            .output_data(always_success_cell_data.pack())
            .build();
        let always_success_out_point = OutPoint::new(always_success_tx.hash(), 0);

        let mut block = BlockBuilder::default()
            .timestamp(unix_time_as_millis().pack())
            .difficulty(U256::from(1000u64).pack())
            .transaction(always_success_tx)
            .build();
        let consensus = Consensus::default()
            .set_genesis_block(block.clone())
            .set_cellbase_maturity(0);

        let (shared, table) = SharedBuilder::default()
            .consensus(consensus)
            .build()
            .unwrap();

        let notify = NotifyService::default().start(Some("tx pool executor"));

        let chain_service = ChainService::new(shared.clone(), table, notify);
        let chain_controller = chain_service.start::<&str>(None);

        for _i in 0..height {
            let number = block.header().number() + 1;
            let timestamp = block.header().timestamp() + 1;

            let last_epoch = shared.get_block_epoch(&block.hash()).unwrap();
            let epoch = shared
                .next_epoch_ext(&last_epoch, &block.header())
                .unwrap_or(last_epoch);

            let outputs = (0..20)
                .map(|_| {
                    CellOutput::new_builder()
                        .capacity(capacity_bytes!(50).pack())
                        .lock(always_success_script.clone())
                        .build()
                })
                .collect::<Vec<_>>();
            let outputs_data = (0..20).map(|_| Bytes::new().pack());
            let cellbase = TransactionBuilder::default()
                .input(CellInput::new_cellbase_input(number))
                .outputs(outputs)
                .outputs_data(outputs_data)
                .build();

            let txs = (10..20).map(|i| {
                TransactionBuilder::default()
                    .input(CellInput::new(OutPoint::new(cellbase.hash(), i), 0))
                    .output(
                        CellOutput::new_builder()
                            .capacity(capacity_bytes!(50).pack())
                            .lock(always_success_script.clone())
                            .build(),
                    )
                    .output_data(Default::default())
                    .cell_dep(
                        CellDep::new_builder()
                            .out_point(always_success_out_point.to_owned())
                            .dep_type(DepType::Code.pack())
                            .build(),
                    )
                    .build()
            });

            block = BlockBuilder::default()
                .parent_hash(block.header().hash().to_owned())
                .number(number.pack())
                .epoch(epoch.number().pack())
                .timestamp(timestamp.pack())
                .difficulty(epoch.difficulty().pack())
                .transaction(cellbase.clone())
                .transactions(txs)
                .build();

            chain_controller
                .process_block(Arc::new(block.clone()), false)
                .expect("process block should be OK");
        }

        (shared, always_success_out_point)
    }

    #[test]
    fn test_verify_and_add_tx_to_pool() {
        let (shared, always_success_out_point) = setup(10);
        let last_block = shared
            .store()
            .get_block(&shared.snapshot().tip_hash())
            .unwrap();
        let last_cellbase = last_block.transactions().first().cloned().unwrap();

        // building 10 txs and broadcast some
        let txs = (0..20u8)
            .map(|i| {
                let data = Bytes::from(vec![i]);
                TransactionBuilder::default()
                    .input(CellInput::new(
                        OutPoint::new(last_cellbase.hash(), u32::from(i)),
                        0,
                    ))
                    .output(
                        CellOutput::new_builder()
                            .capacity(capacity_bytes!(50).pack())
                            .build(),
                    )
                    .output_data(data.pack())
                    .cell_dep(
                        CellDep::new_builder()
                            .out_point(always_success_out_point.to_owned())
                            .dep_type(DepType::Code.pack())
                            .build(),
                    )
                    .build()
            })
            .collect::<Vec<_>>();

        let tx_pool_executor = TxPoolExecutor::new(shared.clone());

        // spent cell
        let result = tx_pool_executor
            .verify_and_add_txs_to_pool(txs[1..=5].to_vec())
            .expect("verify relay tx");
        assert_eq!(result, vec![12; 5]);
        // spent conflict cell
        let result = tx_pool_executor.verify_and_add_txs_to_pool(txs[10..15].to_vec());
        assert_eq!(
            result,
            Err(PoolError::UnresolvableTransaction(UnresolvableError::Dead(
                txs[10].inputs().get(0).unwrap().previous_output()
            )))
        assert_error_eq(
            result.err(),
            Some(OutPointError::DeadCell(txs[10].inputs()[0].previous_output.to_owned()).into()),
        );
        // spent half available half conflict cells
        let result = tx_pool_executor.verify_and_add_txs_to_pool(txs[6..=15].to_vec());
        assert_eq!(
            result,
            Err(PoolError::UnresolvableTransaction(UnresolvableError::Dead(
                txs[10].inputs().get(0).unwrap().previous_output()
            )))
        assert_error_eq(
            result.err(),
            Some(OutPointError::DeadCell(txs[10].inputs()[0].previous_output.to_owned()).into()),
        );
        // spent one duplicate cell
        let result = tx_pool_executor.verify_and_add_tx_to_pool(txs[1].to_owned());
        assert_eq!(result.err(), Some(PoolError::Duplicate));
        // spent one conflict cell
        let result = tx_pool_executor.verify_and_add_tx_to_pool(txs[13].to_owned());
        assert_error_eq(
            result.err(),
            Some(OutPointError::DeadCell(txs[13].inputs()[0].previous_output.to_owned()).into()),
        assert_eq!(
            result,
            Err(PoolError::UnresolvableTransaction(UnresolvableError::Dead(
                txs[13].inputs().get(0).unwrap().previous_output()
            )))
        );
    }

    #[test]
    fn test_verify_and_add_invalid_since_tx_to_pool() {
        let (shared, always_success_out_point) = setup(10);
        let last_block = shared
            .store()
            .get_block(&shared.snapshot().tip_hash())
            .unwrap();
        let last_cellbase = last_block.transactions().first().cloned().unwrap();
        let tip_number = shared.snapshot().tip_number();

        let transactions: Vec<TransactionView> = (tip_number - 1..=tip_number + 2)
            .map(|number| {
                let since = number;
                TransactionBuilder::default()
                    .input(CellInput::new(
                        OutPoint::new(last_cellbase.hash(), 0),
                        since,
                    ))
                    .output(
                        CellOutput::new_builder()
                            .capacity(capacity_bytes!(50).pack())
                            .build(),
                    )
                    .output_data(Default::default())
                    .cell_dep(
                        CellDep::new_builder()
                            .out_point(always_success_out_point.to_owned())
                            .dep_type(DepType::Code.pack())
                            .build(),
                    )
                    .build()
            })
            .collect();

        let tx_pool_executor = TxPoolExecutor::new(shared.clone());

        assert_eq!(
            tx_pool_executor
                .verify_and_add_tx_to_pool(transactions[0].clone())
                .ok(),
            Some(12),
        );
        assert_eq!(
            tx_pool_executor
                .verify_and_add_tx_to_pool(transactions[1].clone())
                .ok(),
            Some(12),
        );
        assert_eq!(
            tx_pool_executor
                .verify_and_add_tx_to_pool(transactions[2].clone())
                .ok(),
            Some(12),
        );
        assert_error_eq(
            tx_pool_executor
                .verify_and_add_tx_to_pool(transactions[3].clone())
                .err(),
            Some(TransactionError::ImmatureTransaction.into()),
        );
    }
}
