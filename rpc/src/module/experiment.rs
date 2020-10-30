use crate::error::RPCError;
use ckb_dao::DaoCalculator;
use ckb_jsonrpc_types::{
    Capacity, DryRunResult, EstimateResult, OutPoint, Script, Transaction, Uint64,
};
use ckb_shared::{shared::Shared, Snapshot};
use ckb_store::ChainStore;
use ckb_types::{
    core::cell::{resolve_transaction, CellProvider, CellStatus, HeaderChecker},
    packed,
    prelude::*,
    H256,
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
    /// Returns the transaction hash for the given transaction.
    ///
    /// ## Examples
    ///
    /// Request
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "method": "_compute_transaction_hash",
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
    ///             "args": "0x",
    ///             "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
    ///             "hash_type": "data"
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
    ///   "result": "0xa0ef4eb5f4ceeb08a4c8524d84c5da95dce2f608e0ca2ec8091191b0f330c6e3"
    /// }
    /// ```
    #[deprecated(
        since = "0.36.0",
        note = "Please implement molecule and compute the transaction hash in clients."
    )]
    #[rpc(name = "_compute_transaction_hash")]
    fn compute_transaction_hash(&self, tx: Transaction) -> Result<H256>;

    /// Returns the script hash for the given script.
    ///
    /// ## Examples
    ///
    /// Request
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "method": "_compute_script_hash",
    ///   "params": [
    ///     {
    ///       "args": "0x",
    ///       "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
    ///       "hash_type": "data"
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
    ///   "result": "0x4ceaa32f692948413e213ce6f3a83337145bde6e11fd8cb94377ce2637dcc412"
    /// }
    /// ```
    #[deprecated(
        since = "0.36.0",
        note = "Please implement molecule and compute the script hash in clients."
    )]
    #[rpc(name = "_compute_script_hash")]
    fn compute_script_hash(&self, script: Script) -> Result<H256>;

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
    ///             "args": "0x",
    ///             "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
    ///             "hash_type": "data"
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
    /// * `out_point` - Reference to the DAO cell.
    /// * `block_hash` - The assumed reference block for withdrawing. This block must be in the
    /// [canonical chain](trait.ChainRpc.html#canonical-chain).
    ///
    /// ## Returns
    ///
    /// The RPC returns the final capacity when the cell `out_point` is withdrawn using the block
    /// `block_hash` as the reference.
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
        block_hash: H256,
    ) -> Result<Capacity>;

    /// Estimates a fee rate (capacity/KB) for a transaction that to be committed within the expect number of blocks.
    #[deprecated(
        since = "0.34.0",
        note = "This method is deprecated because of the performance issue. It always returns an error now."
    )]
    #[rpc(name = "estimate_fee_rate")] // noexample
    fn estimate_fee_rate(&self, expect_confirm_blocks: Uint64) -> Result<EstimateResult>;
}

pub(crate) struct ExperimentRpcImpl {
    pub shared: Shared,
}

impl ExperimentRpc for ExperimentRpcImpl {
    fn compute_transaction_hash(&self, tx: Transaction) -> Result<H256> {
        let tx: packed::Transaction = tx.into();
        Ok(tx.calc_tx_hash().unpack())
    }

    fn compute_script_hash(&self, script: Script) -> Result<H256> {
        let script: packed::Script = script.into();
        Ok(script.calc_script_hash().unpack())
    }

    fn dry_run_transaction(&self, tx: Transaction) -> Result<DryRunResult> {
        let tx: packed::Transaction = tx.into();
        DryRunner::new(&self.shared).run(tx)
    }

    fn calculate_dao_maximum_withdraw(
        &self,
        out_point: OutPoint,
        block_hash: H256,
    ) -> Result<Capacity> {
        let snapshot: &Snapshot = &self.shared.snapshot();
        let consensus = snapshot.consensus();
        let calculator = DaoCalculator::new(consensus, snapshot);
        match calculator.maximum_withdraw(&out_point.into(), &block_hash.pack()) {
            Ok(capacity) => Ok(capacity.into()),
            Err(err) => Err(RPCError::from_ckb_error(err)),
        }
    }

    fn estimate_fee_rate(&self, _expect_confirm_blocks: Uint64) -> Result<EstimateResult> {
        Err(RPCError::custom(
            RPCError::Deprecated,
            "estimate_fee_rate have been deprecated due to it has availability and performance issue"
        ))
    }
}

// DryRunner dry run given transaction, and return the result, including execution cycles.
pub(crate) struct DryRunner<'a> {
    shared: &'a Shared,
}

impl<'a> CellProvider for DryRunner<'a> {
    fn cell(&self, out_point: &packed::OutPoint, with_data: bool) -> CellStatus {
        let snapshot = self.shared.snapshot();
        snapshot
            .get_cell(out_point)
            .map(|mut cell_meta| {
                if with_data {
                    cell_meta.mem_cell_data = snapshot.get_cell_data(out_point);
                }
                CellStatus::live_cell(cell_meta)
            })  // treat as live cell, regardless of live or dead
            .unwrap_or(CellStatus::Unknown)
    }
}

impl<'a> HeaderChecker for DryRunner<'a> {
    fn check_valid(
        &self,
        block_hash: &packed::Byte32,
    ) -> std::result::Result<(), ckb_error::Error> {
        self.shared.snapshot().check_valid(block_hash)
    }
}

impl<'a> DryRunner<'a> {
    pub(crate) fn new(shared: &'a Shared) -> Self {
        Self { shared }
    }

    pub(crate) fn run(&self, tx: packed::Transaction) -> Result<DryRunResult> {
        let snapshot: &Snapshot = &self.shared.snapshot();
        match resolve_transaction(tx.into_view(), &mut HashSet::new(), self, self) {
            Ok(resolved) => {
                let consensus = snapshot.consensus();
                let max_cycles = consensus.max_block_cycles;
                match ScriptVerifier::new(&resolved, snapshot).verify(max_cycles) {
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
