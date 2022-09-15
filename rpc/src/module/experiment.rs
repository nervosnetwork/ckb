use crate::error::RPCError;
use ckb_dao::DaoCalculator;
use ckb_jsonrpc_types::{
    Capacity, DaoWithdrawingCalculationKind, DryRunResult, OutPoint, Transaction,
};
use ckb_shared::{shared::Shared, Snapshot};
use ckb_store::{data_loader_wrapper::AsDataLoader, ChainStore};
use ckb_types::{
    core::{
        self,
        cell::{resolve_transaction, CellProvider, CellStatus, HeaderChecker},
        error::OutPointError,
    },
    packed,
    prelude::*,
};
use ckb_verification::ScriptVerifier;
use jsonrpc_core::Result;
use jsonrpc_derive::rpc;
use std::collections::HashSet;

/// RPC Module Experiment for experimenting methods.
///
/// **EXPERIMENTAL warning**
///
/// The methods here may be removed or changed in future releases without prior notifications.
#[rpc(server)]
pub trait ExperimentRpc {
    /// Dry run a transaction and return the execution cycles.
    ///
    /// This method will not check the transaction validity, but only run the lock script
    /// and type script and then return the execution cycles.
    ///
    /// It is used to debug transaction scripts and query how many cycles the scripts consume.
    ///
    /// ## Errors
    ///
    /// * [`TransactionFailedToResolve (-301)`](../enum.RPCError.html#variant.TransactionFailedToResolve) - Failed to resolve the referenced cells and headers used in the transaction, as inputs or dependencies.
    /// * [`TransactionFailedToVerify (-302)`](../enum.RPCError.html#variant.TransactionFailedToVerify) - There is a script returns with an error.
    ///
    /// ## Examples
    ///
    /// Request
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "method": "dry_run_transaction",
    ///   "params": [
    ///     {
    ///       "cell_deps": [
    ///         {
    ///           "dep_type": "code",
    ///           "out_point": {
    ///             "index": "0x0",
    ///             "tx_hash": "0xa4037a893eb48e18ed4ef61034ce26eba9c585f15c9cee102ae58505565eccc3"
    ///           }
    ///         }
    ///       ],
    ///       "header_deps": [
    ///         "0x7978ec7ce5b507cfb52e149e36b1a23f6062ed150503c85bbf825da3599095ed"
    ///       ],
    ///       "inputs": [
    ///         {
    ///           "previous_output": {
    ///             "index": "0x0",
    ///             "tx_hash": "0x365698b50ca0da75dca2c87f9e7b563811d3b5813736b8cc62cc3b106faceb17"
    ///           },
    ///           "since": "0x0"
    ///         }
    ///       ],
    ///       "outputs": [
    ///         {
    ///           "capacity": "0x2540be400",
    ///           "lock": {
    ///             "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
    ///             "hash_type": "data",
    ///             "args": "0x"
    ///           },
    ///           "type": null
    ///         }
    ///       ],
    ///       "outputs_data": [
    ///         "0x"
    ///       ],
    ///       "version": "0x0",
    ///       "witnesses": []
    ///     }
    ///   ]
    /// }
    /// ```
    ///
    /// Response
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "result": {
    ///     "cycles": "0x219"
    ///   }
    /// }
    /// ```
    #[rpc(name = "dry_run_transaction")]
    fn dry_run_transaction(&self, tx: Transaction) -> Result<DryRunResult>;

    /// Calculates the maximum withdrawal one can get, given a referenced DAO cell, and
    /// a withdrawing block hash.
    ///
    /// ## Params
    ///
    /// * `out_point` - Reference to the DAO cell, the depositing transaction's output.
    /// * `kind` - Two kinds of dao withdrawal amount calculation option.
    ///
    /// option 1, the assumed reference block hash for withdrawing phase 1 transaction, this block must be in the
    /// [canonical chain](trait.ChainRpc.html#canonical-chain), the calculation of occupied capacity will be based on the depositing transaction's output, assuming the output of phase 1 transaction is the same as the depositing transaction's output.
    ///
    /// option 2, the out point of the withdrawing phase 1 transaction, the calculation of occupied capacity will be based on corresponding phase 1 transaction's output.
    ///
    /// ## Returns
    ///
    /// The RPC returns the final capacity when the cell `out_point` is withdrawn using the block hash or withdrawing phase 1 transaction out point as the reference.
    ///
    /// In CKB, scripts cannot get the information about in which block the transaction is
    /// committed. A workaround is letting the transaction reference a block hash so the script
    /// knows that the transaction is committed at least after the reference block.
    ///
    /// ## Errors
    ///
    /// * [`DaoError (-5)`](../enum.RPCError.html#variant.DaoError) - The given out point is not a valid cell for DAO computation.
    /// * [`CKBInternalError (-1)`](../enum.RPCError.html#variant.CKBInternalError) - Mathematics overflow.
    ///
    /// ## Examples
    ///
    /// Request
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "method": "calculate_dao_maximum_withdraw",
    ///   "params": [
    ///     {
    ///       "index": "0x0",
    ///       "tx_hash": "0xa4037a893eb48e18ed4ef61034ce26eba9c585f15c9cee102ae58505565eccc3"
    ///     },
    ///     "0xa5f5c85987a15de25661e5a214f2c1449cd803f071acc7999820f25246471f40"
    ///   ]
    /// }
    /// ```
    ///
    /// Response
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "result": "0x4a8b4e8a4"
    /// }
    /// ```
    #[rpc(name = "calculate_dao_maximum_withdraw")]
    fn calculate_dao_maximum_withdraw(
        &self,
        out_point: OutPoint,
        kind: DaoWithdrawingCalculationKind,
    ) -> Result<Capacity>;
}

pub(crate) struct ExperimentRpcImpl {
    pub shared: Shared,
}

impl ExperimentRpc for ExperimentRpcImpl {
    fn dry_run_transaction(&self, tx: Transaction) -> Result<DryRunResult> {
        let tx: packed::Transaction = tx.into();
        DryRunner::new(&self.shared).run(tx)
    }

    fn calculate_dao_maximum_withdraw(
        &self,
        out_point: OutPoint,
        kind: DaoWithdrawingCalculationKind,
    ) -> Result<Capacity> {
        let snapshot: &Snapshot = &self.shared.snapshot();
        let consensus = snapshot.consensus();
        let out_point: packed::OutPoint = out_point.into();
        let data_loader = snapshot.borrow_as_data_loader();
        let calculator = DaoCalculator::new(consensus, &data_loader);
        match kind {
            DaoWithdrawingCalculationKind::WithdrawingHeaderHash(withdrawing_header_hash) => {
                let (tx, deposit_header_hash) = snapshot
                    .get_transaction(&out_point.tx_hash())
                    .ok_or_else(|| RPCError::invalid_params("invalid out_point"))?;
                let output = tx
                    .outputs()
                    .get(out_point.index().unpack())
                    .ok_or_else(|| RPCError::invalid_params("invalid out_point"))?;
                let output_data = tx
                    .outputs_data()
                    .get(out_point.index().unpack())
                    .ok_or_else(|| RPCError::invalid_params("invalid out_point"))?;

                match calculator.calculate_maximum_withdraw(
                    &output,
                    core::Capacity::bytes(output_data.len()).expect("should not overlfow"),
                    &deposit_header_hash,
                    &withdrawing_header_hash.pack(),
                ) {
                    Ok(capacity) => Ok(capacity.into()),
                    Err(err) => Err(RPCError::custom_with_error(RPCError::DaoError, err)),
                }
            }
            DaoWithdrawingCalculationKind::WithdrawingOutPoint(withdrawing_out_point) => {
                let (_tx, deposit_header_hash) = snapshot
                    .get_transaction(&out_point.tx_hash())
                    .ok_or_else(|| RPCError::invalid_params("invalid out_point"))?;

                let withdrawing_out_point: packed::OutPoint = withdrawing_out_point.into();
                let (withdrawing_tx, withdrawing_header_hash) = snapshot
                    .get_transaction(&withdrawing_out_point.tx_hash())
                    .ok_or_else(|| RPCError::invalid_params("invalid withdrawing_out_point"))?;

                let output = withdrawing_tx
                    .outputs()
                    .get(withdrawing_out_point.index().unpack())
                    .ok_or_else(|| RPCError::invalid_params("invalid withdrawing_out_point"))?;
                let output_data = withdrawing_tx
                    .outputs_data()
                    .get(withdrawing_out_point.index().unpack())
                    .ok_or_else(|| RPCError::invalid_params("invalid withdrawing_out_point"))?;

                match calculator.calculate_maximum_withdraw(
                    &output,
                    core::Capacity::bytes(output_data.len()).expect("should not overlfow"),
                    &deposit_header_hash,
                    &withdrawing_header_hash,
                ) {
                    Ok(capacity) => Ok(capacity.into()),
                    Err(err) => Err(RPCError::custom_with_error(RPCError::DaoError, err)),
                }
            }
        }
    }
}

// DryRunner dry run given transaction, and return the result, including execution cycles.
pub(crate) struct DryRunner<'a> {
    shared: &'a Shared,
}

impl<'a> CellProvider for DryRunner<'a> {
    fn cell(&self, out_point: &packed::OutPoint, eager_load: bool) -> CellStatus {
        let snapshot = self.shared.snapshot();
        snapshot
            .get_cell(out_point)
            .map(|mut cell_meta| {
                if eager_load {
                    if let Some((data, data_hash)) = snapshot.get_cell_data(out_point) {
                        cell_meta.mem_cell_data = Some(data);
                        cell_meta.mem_cell_data_hash = Some(data_hash);
                    }
                }
                CellStatus::live_cell(cell_meta)
            })  // treat as live cell, regardless of live or dead
            .unwrap_or(CellStatus::Unknown)
    }
}

impl<'a> HeaderChecker for DryRunner<'a> {
    fn check_valid(&self, block_hash: &packed::Byte32) -> std::result::Result<(), OutPointError> {
        self.shared.snapshot().check_valid(block_hash)
    }
}

impl<'a> DryRunner<'a> {
    pub(crate) fn new(shared: &'a Shared) -> Self {
        Self { shared }
    }

    pub(crate) fn run(&self, tx: packed::Transaction) -> Result<DryRunResult> {
        let snapshot = self.shared.snapshot();
        let data_loader = snapshot.as_data_loader();
        let consensus = snapshot.consensus();
        match resolve_transaction(tx.into_view(), &mut HashSet::new(), self, self) {
            Ok(resolved) => {
                let max_cycles = consensus.max_block_cycles;
                match ScriptVerifier::new(&resolved, data_loader).verify(max_cycles) {
                    Ok(cycles) => Ok(DryRunResult {
                        cycles: cycles.into(),
                    }),
                    Err(err) => Err(RPCError::custom_with_error(
                        RPCError::TransactionFailedToVerify,
                        err,
                    )),
                }
            }
            Err(err) => Err(RPCError::custom_with_error(
                RPCError::TransactionFailedToResolve,
                err,
            )),
        }
    }
}
