use ckb_test_chain_utils::ckb_testnet_consensus;
use ckb_types::{core, packed, prelude::*};

use crate::module::pool::WellKnownScriptsOnlyValidator;

#[test]
fn test_default_outputs_validator() {
    let consensus = ckb_testnet_consensus();
    let validator = WellKnownScriptsOnlyValidator::new(&consensus, &[], &[]);

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
