use crate::error::RPCError;
use ckb_chain_spec::consensus::Consensus;
use ckb_fee_estimator::FeeRate;
use ckb_jsonrpc_types::{OutputsValidator, Transaction, TxPoolInfo};
use ckb_logger::error;
use ckb_network::PeerIndex;
use ckb_script::IllTransactionChecker;
use ckb_shared::shared::Shared;
use ckb_sync::SyncShared;
use ckb_tx_pool::error::Reject;
use ckb_types::{core, packed, prelude::*, H256};
use ckb_verification::{Since, SinceMetric};
use jsonrpc_core::Result;
use jsonrpc_derive::rpc;
use std::convert::TryInto;
use std::sync::Arc;

/// RPC Module Pool for transaction memory pool.
#[rpc(server)]
pub trait PoolRpc {
    /// Submits a new transaction into the transaction pool.
    ///
    /// ## Params
    ///
    /// * `transaction` - The transaction.
    /// * `outputs_validator` - Validates the transaction outputs before entering the tx-pool. (**Optional**, default is "passthrough").
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
}

pub(crate) struct PoolRpcImpl {
    sync_shared: Arc<SyncShared>,
    shared: Shared,
    min_fee_rate: FeeRate,
    reject_ill_transactions: bool,
}

impl PoolRpcImpl {
    pub fn new(
        shared: Shared,
        sync_shared: Arc<SyncShared>,
        min_fee_rate: FeeRate,
        reject_ill_transactions: bool,
    ) -> PoolRpcImpl {
        PoolRpcImpl {
            sync_shared,
            shared,
            min_fee_rate,
            reject_ill_transactions,
        }
    }
}

impl PoolRpc for PoolRpcImpl {
    fn send_transaction(
        &self,
        tx: Transaction,
        outputs_validator: Option<OutputsValidator>,
    ) -> Result<H256> {
        let tx: packed::Transaction = tx.into();
        let tx: core::TransactionView = tx.into_view();

        if let Err(e) = match outputs_validator {
            Some(OutputsValidator::Default) => {
                DefaultOutputsValidator::new(self.shared.consensus()).validate(&tx)
            }
            Some(OutputsValidator::Passthrough) | None => Ok(()),
        } {
            return Err(RPCError::custom_with_data(
                RPCError::PoolRejectedTransactionByOutputsValidator,
                format!(
                    "The transction is rejected by OutputsValidator set in params[1]: {}. \
                    Please set it to passthrough if you really want to send transactions with advanced scripts.",
                    outputs_validator.unwrap_or(OutputsValidator::Default).json_display()
                ),
                e,
            ));
        }

        if self.reject_ill_transactions {
            if let Err(e) = IllTransactionChecker::new(&tx).check() {
                return Err(RPCError::custom_with_data(
                    RPCError::PoolRejectedTransactionByIllTransactionChecker,
                    "The transaction is rejected by IllTransactionChecker",
                    e,
                ));
            }
        }

        let tx_pool = self.shared.tx_pool_controller();
        let submit_txs = tx_pool.submit_txs(vec![tx.clone()]);

        if let Err(e) = submit_txs {
            error!("send submit_txs request error {}", e);
            return Err(RPCError::ckb_internal_error(e));
        }

        let broadcast = |tx_hash: packed::Byte32| {
            // workaround: we are using `PeerIndex(usize::max)` to indicate that tx hash source is itself.
            let peer_index = PeerIndex::new(usize::max_value());
            self.sync_shared
                .state()
                .tx_hashes()
                .entry(peer_index)
                .or_default()
                .insert(tx_hash);
        };
        let tx_hash = tx.hash();
        match submit_txs.unwrap() {
            Ok(_) => {
                broadcast(tx_hash.clone());
                Ok(tx_hash.unpack())
            }
            Err(e) => match RPCError::downcast_submit_transaction_reject(&e) {
                Some(reject) => {
                    if let Reject::Duplicated(_) = reject {
                        broadcast(tx_hash);
                    }
                    Err(RPCError::from_submit_transaction_reject(reject))
                }
                None => Err(RPCError::from_ckb_error(e)),
            },
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
}

struct DefaultOutputsValidator<'a> {
    consensus: &'a Consensus,
}

#[derive(Debug)]
enum DefaultOutputsValidatorError {
    HashType,
    CodeHash,
    ArgsLen,
    ArgsSince,
}

impl<'a> DefaultOutputsValidator<'a> {
    pub fn new(consensus: &'a Consensus) -> Self {
        Self { consensus }
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
    }

    fn validate_type_script(
        &self,
        output: &packed::CellOutput,
    ) -> std::result::Result<(), DefaultOutputsValidatorError> {
        self.validate_dao(output)
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

#[cfg(test)]
mod tests {
    use super::*;
    use ckb_test_chain_utils::ckb_testnet_consensus;
    use ckb_types::{core, packed};

    #[test]
    fn test_default_outputs_validator() {
        let consensus = ckb_testnet_consensus();
        let validator = DefaultOutputsValidator::new(&consensus);

        {
            let type_hash = consensus
                .secp256k1_blake160_sighash_all_type_hash()
                .unwrap();
            // valid output lock
            let tx = build_tx(&type_hash, core::ScriptHashType::Type, vec![1; 20]);
            assert!(validator.validate(&tx).is_ok());

            // invalid args len
            let tx = build_tx(&type_hash, core::ScriptHashType::Type, vec![1; 19]);
            assert!(validator.validate(&tx).is_err());

            // invalid hash type
            let tx = build_tx(&type_hash, core::ScriptHashType::Data, vec![1; 20]);
            assert!(validator.validate(&tx).is_err());

            // invalid code hash
            let tx = build_tx(
                &consensus.dao_type_hash().unwrap(),
                core::ScriptHashType::Type,
                vec![1; 20],
            );
            assert!(validator.validate(&tx).is_err());
        }

        {
            let type_hash = consensus
                .secp256k1_blake160_multisig_all_type_hash()
                .unwrap();
            // valid output lock
            let tx = build_tx(&type_hash, core::ScriptHashType::Type, vec![1; 20]);
            assert!(validator.validate(&tx).is_ok());

            // valid output lock
            let since: u64 = (0b1100_0000 << 56) | 42; // relative timestamp 42 seconds
            let mut args = vec![1; 20];
            args.extend_from_slice(&since.to_le_bytes());
            let tx = build_tx(&type_hash, core::ScriptHashType::Type, args);
            assert!(validator.validate(&tx).is_ok());

            // invalid args len
            let tx = build_tx(&type_hash, core::ScriptHashType::Type, vec![1; 19]);
            assert!(validator.validate(&tx).is_err());

            // invalid hash type
            let tx = build_tx(&type_hash, core::ScriptHashType::Data, vec![1; 20]);
            assert!(validator.validate(&tx).is_err());

            // invalid since args format
            let tx = build_tx(&type_hash, core::ScriptHashType::Type, vec![1; 28]);
            assert!(validator.validate(&tx).is_err());
        }

        {
            let lock_type_hash = consensus
                .secp256k1_blake160_multisig_all_type_hash()
                .unwrap();
            let type_type_hash = consensus.dao_type_hash().unwrap();
            // valid output lock
            let tx = build_tx_with_type(
                &lock_type_hash,
                core::ScriptHashType::Type,
                vec![1; 20],
                &type_type_hash,
                core::ScriptHashType::Type,
            );
            assert!(validator.validate(&tx).is_ok());

            // valid output lock
            let since: u64 = (0b0010_0000 << 56) | 42; // absolute epoch
            let mut args = vec![1; 20];
            args.extend_from_slice(&since.to_le_bytes());
            let tx = build_tx_with_type(
                &lock_type_hash,
                core::ScriptHashType::Type,
                args,
                &type_type_hash,
                core::ScriptHashType::Type,
            );
            assert!(validator.validate(&tx).is_ok());

            // invalid since arg lock
            let since: u64 = (0b1100_0000 << 56) | 42; // relative timestamp 42 seconds
            let mut args = vec![1; 20];
            args.extend_from_slice(&since.to_le_bytes());
            let tx = build_tx_with_type(
                &lock_type_hash,
                core::ScriptHashType::Type,
                args,
                &type_type_hash,
                core::ScriptHashType::Type,
            );
            assert!(validator.validate(&tx).is_err());

            // invalid since args type
            let tx = build_tx_with_type(
                &lock_type_hash,
                core::ScriptHashType::Type,
                vec![1; 20],
                &type_type_hash,
                core::ScriptHashType::Data,
            );
            assert!(validator.validate(&tx).is_err());

            // invalid code hash
            let tx = build_tx_with_type(
                &lock_type_hash,
                core::ScriptHashType::Type,
                vec![1; 20],
                &lock_type_hash,
                core::ScriptHashType::Type,
            );
            assert!(validator.validate(&tx).is_err());
        }
    }

    fn build_tx(
        code_hash: &packed::Byte32,
        hash_type: core::ScriptHashType,
        args: Vec<u8>,
    ) -> core::TransactionView {
        let lock = packed::ScriptBuilder::default()
            .code_hash(code_hash.clone())
            .hash_type(hash_type.into())
            .args(args.pack())
            .build();
        core::TransactionBuilder::default()
            .output(packed::CellOutput::new_builder().lock(lock).build())
            .build()
    }

    fn build_tx_with_type(
        lock_code_hash: &packed::Byte32,
        lock_hash_type: core::ScriptHashType,
        lock_args: Vec<u8>,
        type_code_hash: &packed::Byte32,
        type_hash_type: core::ScriptHashType,
    ) -> core::TransactionView {
        let lock = packed::ScriptBuilder::default()
            .code_hash(lock_code_hash.clone())
            .hash_type(lock_hash_type.into())
            .args(lock_args.pack())
            .build();
        let type_ = packed::ScriptBuilder::default()
            .code_hash(type_code_hash.clone())
            .hash_type(type_hash_type.into())
            .build();
        core::TransactionBuilder::default()
            .output(
                packed::CellOutput::new_builder()
                    .lock(lock)
                    .type_(Some(type_).pack())
                    .build(),
            )
            .build()
    }
}
