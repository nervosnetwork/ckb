use crate::error::RPCError;
use ckb_chain_spec::consensus::Consensus;
use ckb_jsonrpc_types::{Transaction, TxPoolInfo};
use ckb_logger::error;
use ckb_network::PeerIndex;
use ckb_script::IllTransactionChecker;
use ckb_shared::shared::Shared;
use ckb_sync::SyncShared;
use ckb_tx_pool::{error::SubmitTxError, FeeRate};
use ckb_types::{core, packed, prelude::*, H256};
use ckb_verification::{Since, SinceMetric};
use jsonrpc_core::{Error, Result};
use jsonrpc_derive::rpc;
use serde::{Deserialize, Serialize};
use std::convert::TryInto;
use std::sync::Arc;

#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OutputsValidator {
    Default,
    Passthrough,
}

#[rpc(server)]
pub trait PoolRpc {
    // curl -d '{"id": 2, "jsonrpc": "2.0", "method":"send_transaction","params": [{"version":2, "deps":[], "inputs":[], "outputs":[]}]}' -H 'content-type:application/json' 'http://localhost:8114'
    #[rpc(name = "send_transaction")]
    fn send_transaction(
        &self,
        _tx: Transaction,
        _outputs_validator: Option<OutputsValidator>,
    ) -> Result<H256>;

    // curl -d '{"params": [], "method": "tx_pool_info", "jsonrpc": "2.0", "id": 2}' -H 'content-type:application/json' http://localhost:8114
    #[rpc(name = "tx_pool_info")]
    fn tx_pool_info(&self) -> Result<TxPoolInfo>;
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
            return Err(RPCError::custom(RPCError::Invalid, e));
        }

        if self.reject_ill_transactions {
            if let Err(e) = IllTransactionChecker::new(&tx).check() {
                return Err(RPCError::custom(RPCError::Invalid, format!("{:#}", e)));
            }
        }

        let tx_pool = self.shared.tx_pool_controller();
        let submit_txs = tx_pool.submit_txs(vec![tx.clone()]);

        if let Err(e) = submit_txs {
            error!("send submit_txs request error {}", e);
            return Err(Error::internal_error());
        }

        match submit_txs.unwrap() {
            Ok(_) => {
                // workaround: we are using `PeerIndex(usize::max)` to indicate that tx hash source is itself.
                let peer_index = PeerIndex::new(usize::max_value());
                let hash = tx.hash();
                self.sync_shared
                    .state()
                    .tx_hashes()
                    .entry(peer_index)
                    .or_default()
                    .insert(hash.clone());
                Ok(hash.unpack())
            }
            Err(e) => {
                if let Some(e) = e.downcast_ref::<SubmitTxError>() {
                    match *e {
                        SubmitTxError::LowFeeRate(min_fee) => {
                            return Err(RPCError::custom(
                                RPCError::Invalid,
                                format!(
                                    "transaction fee rate lower than min_fee_rate: {} shannons/KB, min fee for current tx: {}",
                                    self.min_fee_rate, min_fee,
                                ),
                            ));
                        }
                        SubmitTxError::ExceededMaximumAncestorsCount => {
                            return Err(RPCError::custom(
                                RPCError::Invalid,
                                    "transaction exceeded maximum ancestors count limit, try send it later".to_string(),
                            ));
                        }
                    }
                }
                Err(RPCError::custom(RPCError::Invalid, format!("{:#}", e)))
            }
        }
    }

    fn tx_pool_info(&self) -> Result<TxPoolInfo> {
        let tx_pool = self.shared.tx_pool_controller();
        let get_tx_pool_info = tx_pool.get_tx_pool_info();
        if let Err(e) = get_tx_pool_info {
            error!("send get_tx_pool_info request error {}", e);
            return Err(Error::internal_error());
        };

        let tx_pool_info = get_tx_pool_info.unwrap();

        Ok(TxPoolInfo {
            pending: (tx_pool_info.pending_size as u64).into(),
            proposed: (tx_pool_info.proposed_size as u64).into(),
            orphan: (tx_pool_info.orphan_size as u64).into(),
            total_tx_size: (tx_pool_info.total_tx_size as u64).into(),
            total_tx_cycles: tx_pool_info.total_tx_cycles.into(),
            min_fee_rate: self.min_fee_rate.as_u64().into(),
            last_txs_updated_at: tx_pool_info.last_txs_updated_at.into(),
        })
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
