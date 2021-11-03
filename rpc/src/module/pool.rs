use crate::error::RPCError;
use ckb_chain_spec::consensus::Consensus;
use ckb_constant::hardfork::{mainnet, testnet};
use ckb_jsonrpc_types::{OutputsValidator, RawTxPool, Script, Transaction, TxPoolInfo};
use ckb_logger::error;
use ckb_script::IllTransactionChecker;
use ckb_shared::{shared::Shared, Snapshot};
use ckb_types::{core, packed, prelude::*, H256};
use ckb_verification::{Since, SinceMetric, TxVerifyEnv};
use jsonrpc_core::Result;
use jsonrpc_derive::rpc;
use std::convert::TryInto;
use std::sync::Arc;

/// RPC Module Pool for transaction memory pool.
#[rpc(server)]
pub trait PoolRpc {
    /// Submits a new transaction into the transaction pool. If the transaction is already in the
    /// pool, rebroadcast it to peers.
    ///
    /// ## Params
    ///
    /// * `transaction` - The transaction.
    /// * `outputs_validator` - Validates the transaction outputs before entering the tx-pool. (**Optional**, default is "well_known_scripts_only").
    ///
    /// ## Errors
    ///
    /// * [`PoolRejectedTransactionByOutputsValidator (-1102)`](../enum.RPCError.html#variant.PoolRejectedTransactionByOutputsValidator) - The transaction is rejected by the validator specified by `outputs_validator`. If you really want to send transactions with advanced scripts, please set `outputs_validator` to "passthrough".
    /// * [`PoolRejectedTransactionByIllTransactionChecker (-1103)`](../enum.RPCError.html#variant.PoolRejectedTransactionByIllTransactionChecker) - Pool rejects some transactions which seem contain invalid VM instructions. See the issue link in the error message for details.
    /// * [`PoolRejectedTransactionByMinFeeRate (-1104)`](../enum.RPCError.html#variant.PoolRejectedTransactionByMinFeeRate) - The transaction fee rate must be greater than or equal to the config option `tx_pool.min_fee_rate`.
    /// * [`PoolRejectedTransactionByMaxAncestorsCountLimit (-1105)`](../enum.RPCError.html#variant.PoolRejectedTransactionByMaxAncestorsCountLimit) - The ancestors count must be greater than or equal to the config option `tx_pool.max_ancestors_count`.
    /// * [`PoolIsFull (-1106)`](../enum.RPCError.html#variant.PoolIsFull) - Pool is full.
    /// * [`PoolRejectedDuplicatedTransaction (-1107)`](../enum.RPCError.html#variant.PoolRejectedDuplicatedTransaction) - The transaction is already in the pool.
    /// * [`TransactionFailedToResolve (-301)`](../enum.RPCError.html#variant.TransactionFailedToResolve) - Failed to resolve the referenced cells and headers used in the transaction, as inputs or dependencies.
    /// * [`TransactionFailedToVerify (-302)`](../enum.RPCError.html#variant.TransactionFailedToVerify) - Failed to verify the transaction.
    ///
    /// ## Examples
    ///
    /// Request
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "method": "send_transaction",
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
    ///     },
    ///     "passthrough"
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
    #[rpc(name = "send_transaction")]
    fn send_transaction(
        &self,
        tx: Transaction,
        outputs_validator: Option<OutputsValidator>,
    ) -> Result<H256>;

    /// Returns the transaction pool information.
    ///
    /// ## Examples
    ///
    /// Request
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "method": "tx_pool_info",
    ///   "params": []
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
    ///     "last_txs_updated_at": "0x0",
    ///     "min_fee_rate": "0x0",
    ///     "orphan": "0x0",
    ///     "pending": "0x1",
    ///     "proposed": "0x0",
    ///     "tip_hash": "0xa5f5c85987a15de25661e5a214f2c1449cd803f071acc7999820f25246471f40",
    ///     "tip_number": "0x400",
    ///     "total_tx_cycles": "0x219",
    ///     "total_tx_size": "0x112"
    ///   }
    /// }
    /// ```
    #[rpc(name = "tx_pool_info")]
    fn tx_pool_info(&self) -> Result<TxPoolInfo>;

    /// Removes all transactions from the transaction pool.
    ///
    /// ## Examples
    ///
    /// Request
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "method": "clear_tx_pool",
    ///   "params": []
    /// }
    /// ```
    ///
    /// Response
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "result": null
    /// }
    /// ```
    #[rpc(name = "clear_tx_pool")]
    fn clear_tx_pool(&self) -> Result<()>;

    /// Returns all transaction ids in tx pool as a json array of string transaction ids.
    /// ## Params
    ///
    /// * `verbose` - True for a json object, false for array of transaction ids, default=false
    ///
    /// ## Examples
    ///
    /// Request
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "method": "get_raw_tx_pool",
    ///   "params": [true]
    /// }
    /// ```
    ///
    /// Response
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "result":
    ///    {
    ///        "pending": {
    ///            "0xa0ef4eb5f4ceeb08a4c8524d84c5da95dce2f608e0ca2ec8091191b0f330c6e3": {
    ///                "cycles": "0x219",
    ///                "size": "0x112",
    ///                "fee": "0x16923f7dcf",
    ///                "ancestors_size": "0x112",
    ///                "ancestors_cycles": "0x219",
    ///                "ancestors_count": "0x1",
    ///                "timestamp": "0x17c983e6e44"
    ///            }
    ///        },
    ///        "proposed": {}
    ///    }
    /// }
    /// ```
    #[rpc(name = "get_raw_tx_pool")]
    fn get_raw_tx_pool(&self, verbose: Option<bool>) -> Result<RawTxPool>;

    /// Returns whether tx-pool service is started, ready for request.
    ///
    /// ## Examples
    ///
    /// Request
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "method": "tx_pool_ready",
    ///   "params": []
    /// }
    /// ```
    ///
    /// Response
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "result": true
    /// }
    /// ```
    #[rpc(name = "tx_pool_ready")]
    fn tx_pool_ready(&self) -> Result<bool>;
}

pub(crate) struct PoolRpcImpl {
    shared: Shared,
    min_fee_rate: core::FeeRate,
    reject_ill_transactions: bool,
    well_known_lock_scripts: Vec<packed::Script>,
    well_known_type_scripts: Vec<packed::Script>,
}

impl PoolRpcImpl {
    pub fn new(
        shared: Shared,
        min_fee_rate: core::FeeRate,
        reject_ill_transactions: bool,
        mut extra_well_known_lock_scripts: Vec<packed::Script>,
        mut extra_well_known_type_scripts: Vec<packed::Script>,
    ) -> PoolRpcImpl {
        let mut well_known_lock_scripts =
            build_well_known_lock_scripts(shared.consensus().id.as_str());
        let mut well_known_type_scripts =
            build_well_known_type_scripts(shared.consensus().id.as_str());

        well_known_lock_scripts.append(&mut extra_well_known_lock_scripts);
        well_known_type_scripts.append(&mut extra_well_known_type_scripts);

        PoolRpcImpl {
            shared,
            min_fee_rate,
            reject_ill_transactions,
            well_known_lock_scripts,
            well_known_type_scripts,
        }
    }
}

/// Build well known lock scripts
/// https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0024-ckb-system-script-list/0024-ckb-system-script-list.md
/// 1. anyone_can_pay
/// 2. cheque
fn build_well_known_lock_scripts(chain_spec_name: &str) -> Vec<packed::Script> {
    serde_json::from_str::<Vec<Script>>(
    match chain_spec_name {
        mainnet::CHAIN_SPEC_NAME => {
            r#"
            [
                {
                    "code_hash": "0xd369597ff47f29fbc0d47d2e3775370d1250b85140c670e4718af712983a2354",
                    "hash_type": "type",
                    "args": "0x"
                },
                {
                    "code_hash": "0xe4d4ecc6e5f9a059bf2f7a82cca292083aebc0c421566a52484fe2ec51a9fb0c",
                    "hash_type": "type",
                    "args": "0x"
                }
            ]
            "#
        }
        testnet::CHAIN_SPEC_NAME => {
            r#"
            [
                {
                    "code_hash": "0x3419a1c09eb2567f6552ee7a8ecffd64155cffe0f1796e6e61ec088d740c1356",
                    "hash_type": "type",
                    "args": "0x"
                },
                {
                    "code_hash": "0x60d5f39efce409c587cb9ea359cefdead650ca128f0bd9cb3855348f98c70d5b",
                    "hash_type": "type",
                    "args": "0x"
                }
            ]
            "#
        }
        _ => "[]"
    }).expect("checked json str").into_iter().map(Into::into).collect()
}

/// Build well known type scripts
/// https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0024-ckb-system-script-list/0024-ckb-system-script-list.md
/// 1. Simple UDT
fn build_well_known_type_scripts(chain_spec_name: &str) -> Vec<packed::Script> {
    serde_json::from_str::<Vec<Script>>(
    match chain_spec_name {
        mainnet::CHAIN_SPEC_NAME => {
            r#"
            [
                {
                    "code_hash": "0x5e7a36a77e68eecc013dfa2fe6a23f3b6c344b04005808694ae6dd45eea4cfd5",
                    "hash_type": "type",
                    "args": "0x"
                }
            ]
            "#
        }
        testnet::CHAIN_SPEC_NAME => {
            r#"
            [
                {
                    "code_hash": "0xc5e5dcf215925f7ef4dfaf5f4b4f105bc321c02776d6e7d52a1db3fcd9d011a4",
                    "hash_type": "type",
                    "args": "0x"
                }
            ]
            "#
        }
        _ => "[]"
    }).expect("checked json str").into_iter().map(Into::into).collect()
}

impl PoolRpc for PoolRpcImpl {
    fn tx_pool_ready(&self) -> Result<bool> {
        let tx_pool = self.shared.tx_pool_controller();
        Ok(tx_pool.service_started())
    }

    fn send_transaction(
        &self,
        tx: Transaction,
        outputs_validator: Option<OutputsValidator>,
    ) -> Result<H256> {
        let tx: packed::Transaction = tx.into();
        let tx: core::TransactionView = tx.into_view();

        if let Err(e) = match outputs_validator {
            Some(OutputsValidator::WellKnownScriptsOnly) | None => {
                WellKnownScriptsOnlyValidator::new(
                    self.shared.consensus(),
                    &self.well_known_lock_scripts,
                    &self.well_known_type_scripts,
                )
                .validate(&tx)
            }
            Some(OutputsValidator::Passthrough) => Ok(()),
        } {
            return Err(RPCError::custom_with_data(
                RPCError::PoolRejectedTransactionByOutputsValidator,
                format!(
                    "The transaction is rejected by OutputsValidator set in params[1]: {}. \
                    Please check the related information in https://github.com/nervosnetwork/ckb/wiki/Transaction-%C2%BB-Default-Outputs-Validator",
                    outputs_validator.unwrap_or(OutputsValidator::WellKnownScriptsOnly).json_display()
                ),
                e,
            ));
        }

        if self.reject_ill_transactions {
            let snapshot: &Snapshot = &self.shared.snapshot();
            let consensus = snapshot.consensus();
            let tx_env = {
                let tip_header = snapshot.tip_header();
                TxVerifyEnv::new_submit(&tip_header)
            };
            if let Err(e) = IllTransactionChecker::new(&tx, consensus, &tx_env).check() {
                return Err(RPCError::custom_with_data(
                    RPCError::PoolRejectedTransactionByIllTransactionChecker,
                    "The transaction is rejected by IllTransactionChecker",
                    e,
                ));
            }
        }

        let tx_pool = self.shared.tx_pool_controller();
        let submit_tx = tx_pool.submit_local_tx(tx.clone());

        if let Err(e) = submit_tx {
            error!("send submit_tx request error {}", e);
            return Err(RPCError::ckb_internal_error(e));
        }

        let tx_hash = tx.hash();
        match submit_tx.unwrap() {
            Ok(_) => Ok(tx_hash.unpack()),
            Err(reject) => Err(RPCError::from_submit_transaction_reject(&reject)),
        }
    }

    fn tx_pool_info(&self) -> Result<TxPoolInfo> {
        let tx_pool = self.shared.tx_pool_controller();
        let get_tx_pool_info = tx_pool.get_tx_pool_info();
        if let Err(e) = get_tx_pool_info {
            error!("send get_tx_pool_info request error {}", e);
            return Err(RPCError::ckb_internal_error(e));
        };

        let tx_pool_info = get_tx_pool_info.unwrap();

        Ok(TxPoolInfo {
            tip_hash: tx_pool_info.tip_hash.unpack(),
            tip_number: tx_pool_info.tip_number.into(),
            pending: (tx_pool_info.pending_size as u64).into(),
            proposed: (tx_pool_info.proposed_size as u64).into(),
            orphan: (tx_pool_info.orphan_size as u64).into(),
            total_tx_size: (tx_pool_info.total_tx_size as u64).into(),
            total_tx_cycles: tx_pool_info.total_tx_cycles.into(),
            min_fee_rate: self.min_fee_rate.as_u64().into(),
            last_txs_updated_at: tx_pool_info.last_txs_updated_at.into(),
        })
    }

    fn clear_tx_pool(&self) -> Result<()> {
        let snapshot = Arc::clone(&self.shared.snapshot());
        let tx_pool = self.shared.tx_pool_controller();
        tx_pool
            .clear_pool(snapshot)
            .map_err(|err| RPCError::custom(RPCError::Invalid, err.to_string()))?;

        Ok(())
    }

    fn get_raw_tx_pool(&self, verbose: Option<bool>) -> Result<RawTxPool> {
        let tx_pool = self.shared.tx_pool_controller();

        let raw = if verbose.unwrap_or(false) {
            let info = tx_pool
                .get_all_entry_info()
                .map_err(|err| RPCError::custom(RPCError::CKBInternalError, err.to_string()))?;
            RawTxPool::Verbose(info.into())
        } else {
            let ids = tx_pool
                .get_all_ids()
                .map_err(|err| RPCError::custom(RPCError::CKBInternalError, err.to_string()))?;
            RawTxPool::Ids(ids.into())
        };
        Ok(raw)
    }
}

pub(crate) struct WellKnownScriptsOnlyValidator<'a> {
    consensus: &'a Consensus,
    well_known_lock_scripts: &'a [packed::Script],
    well_known_type_scripts: &'a [packed::Script],
}

#[derive(Debug)]
enum DefaultOutputsValidatorError {
    HashType,
    CodeHash,
    ArgsLen,
    ArgsSince,
    NotWellKnownLockScript,
    NotWellKnownTypeScript,
}

impl<'a> WellKnownScriptsOnlyValidator<'a> {
    pub fn new(
        consensus: &'a Consensus,
        well_known_lock_scripts: &'a [packed::Script],
        well_known_type_scripts: &'a [packed::Script],
    ) -> Self {
        Self {
            consensus,
            well_known_lock_scripts,
            well_known_type_scripts,
        }
    }

    pub fn validate(&self, tx: &core::TransactionView) -> std::result::Result<(), String> {
        tx.outputs()
            .into_iter()
            .enumerate()
            .try_for_each(|(index, output)| {
                self.validate_lock_script(&output)
                    .and(self.validate_type_script(&output))
                    .map_err(|err| format!("output index: {}, error: {:?}", index, err))
            })
    }

    fn validate_lock_script(
        &self,
        output: &packed::CellOutput,
    ) -> std::result::Result<(), DefaultOutputsValidatorError> {
        self.validate_secp256k1_blake160_sighash_all(output)
            .or_else(|_| self.validate_secp256k1_blake160_multisig_all(output))
            .or_else(|_| self.validate_well_known_lock_scripts(output))
    }

    fn validate_type_script(
        &self,
        output: &packed::CellOutput,
    ) -> std::result::Result<(), DefaultOutputsValidatorError> {
        self.validate_dao(output)
            .or_else(|_| self.validate_well_known_type_scripts(output))
    }

    fn validate_secp256k1_blake160_sighash_all(
        &self,
        output: &packed::CellOutput,
    ) -> std::result::Result<(), DefaultOutputsValidatorError> {
        let script = output.lock();
        if !script.is_hash_type_type() {
            Err(DefaultOutputsValidatorError::HashType)
        } else if script.code_hash()
            != self
                .consensus
                .secp256k1_blake160_sighash_all_type_hash()
                .expect("No secp256k1_blake160_sighash_all system cell")
        {
            Err(DefaultOutputsValidatorError::CodeHash)
        } else if script.args().len() != BLAKE160_LEN {
            Err(DefaultOutputsValidatorError::ArgsLen)
        } else {
            Ok(())
        }
    }

    fn validate_secp256k1_blake160_multisig_all(
        &self,
        output: &packed::CellOutput,
    ) -> std::result::Result<(), DefaultOutputsValidatorError> {
        let script = output.lock();
        if !script.is_hash_type_type() {
            Err(DefaultOutputsValidatorError::HashType)
        } else if script.code_hash()
            != self
                .consensus
                .secp256k1_blake160_multisig_all_type_hash()
                .expect("No secp256k1_blake160_multisig_all system cell")
        {
            Err(DefaultOutputsValidatorError::CodeHash)
        } else if script.args().len() != BLAKE160_LEN {
            if script.args().len() == BLAKE160_LEN + SINCE_LEN {
                if extract_since_from_secp256k1_blake160_multisig_all_args(&script).flags_is_valid()
                {
                    Ok(())
                } else {
                    Err(DefaultOutputsValidatorError::ArgsSince)
                }
            } else {
                Err(DefaultOutputsValidatorError::ArgsLen)
            }
        } else {
            Ok(())
        }
    }

    fn validate_well_known_lock_scripts(
        &self,
        output: &packed::CellOutput,
    ) -> std::result::Result<(), DefaultOutputsValidatorError> {
        let script = output.lock();
        if self
            .well_known_lock_scripts
            .iter()
            .any(|well_known_script| is_well_known_script(&script, well_known_script))
        {
            Ok(())
        } else {
            Err(DefaultOutputsValidatorError::NotWellKnownLockScript)
        }
    }

    fn validate_dao(
        &self,
        output: &packed::CellOutput,
    ) -> std::result::Result<(), DefaultOutputsValidatorError> {
        match output.type_().to_opt() {
            Some(script) => {
                if !script.is_hash_type_type() {
                    Err(DefaultOutputsValidatorError::HashType)
                } else if script.code_hash()
                    != self.consensus.dao_type_hash().expect("No dao system cell")
                {
                    Err(DefaultOutputsValidatorError::CodeHash)
                } else if output.lock().args().len() == BLAKE160_LEN + SINCE_LEN {
                    // https://github.com/nervosnetwork/ckb/wiki/Common-Gotchas#nervos-dao
                    let since =
                        extract_since_from_secp256k1_blake160_multisig_all_args(&output.lock());
                    match since.extract_metric() {
                        Some(SinceMetric::EpochNumberWithFraction(_)) if since.is_absolute() => {
                            Ok(())
                        }
                        _ => Err(DefaultOutputsValidatorError::ArgsSince),
                    }
                } else {
                    Ok(())
                }
            }
            None => Ok(()),
        }
    }

    fn validate_well_known_type_scripts(
        &self,
        output: &packed::CellOutput,
    ) -> std::result::Result<(), DefaultOutputsValidatorError> {
        if let Some(script) = output.type_().to_opt() {
            if self
                .well_known_type_scripts
                .iter()
                .any(|well_known_script| is_well_known_script(&script, well_known_script))
            {
                Ok(())
            } else {
                Err(DefaultOutputsValidatorError::NotWellKnownTypeScript)
            }
        } else {
            Ok(())
        }
    }
}

const BLAKE160_LEN: usize = 20;
const SINCE_LEN: usize = 8;

fn extract_since_from_secp256k1_blake160_multisig_all_args(script: &packed::Script) -> Since {
    Since(u64::from_le_bytes(
        (&script.args().raw_data()[BLAKE160_LEN..])
            .try_into()
            .expect("checked len"),
    ))
}

fn is_well_known_script(script: &packed::Script, well_known_script: &packed::Script) -> bool {
    script.hash_type() == well_known_script.hash_type()
        && script.code_hash() == well_known_script.code_hash()
        && script
            .args()
            .as_slice()
            .starts_with(well_known_script.args().as_slice())
}
