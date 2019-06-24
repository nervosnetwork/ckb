use crate::{Address, LiveCellInfo};
use bytes::Bytes;
use ckb_core::{
    block::Block,
    header::Header,
    transaction::{CellOutPoint, CellOutput, OutPoint, TransactionBuilder},
    Capacity,
};
use ckb_crypto::secp::SECP256K1;
use ckb_hash::blake2b_256;
use ckb_jsonrpc_types::Transaction as RpcTransaction;
use numext_fixed_hash::{h256, H256};

pub const ONE_CKB: u64 = 100_000_000;
// H256(secp code hash) + H160 (secp pubkey hash) + u64(capacity) = 32 + 20 + 8 = 60
pub const MIN_SECP_CELL_CAPACITY: u64 = 60 * ONE_CKB;

const SECP_CODE_HASH: H256 =
    h256!("0x94334bdda40b69bae067d84937aa6bbccf8acd0df6626d4b9ac70d4612a11933");

#[derive(Debug, Clone)]
pub struct GenesisInfo {
    header: Header,
    out_points: Vec<Vec<CellOutPoint>>,
    secp_code_hash: H256,
}

impl GenesisInfo {
    pub fn from_block(genesis_block: &Block) -> Result<GenesisInfo, String> {
        let header = genesis_block.header().clone();
        if header.number() != 0 {
            return Err(format!(
                "Convert to GenesisInfo failed, block number {} > 0",
                header.number()
            ));
        }

        let mut secp_code_hash = None;
        let out_points = genesis_block
            .transactions()
            .iter()
            .enumerate()
            .map(|(tx_index, tx)| {
                tx.outputs()
                    .iter()
                    .enumerate()
                    .map(|(index, output)| {
                        if tx_index == 0 && index == 1 {
                            let code_hash = H256::from_slice(&blake2b_256(&output.data))
                                .expect("Convert to H256 error");
                            if code_hash != SECP_CODE_HASH {
                                log::error!(
                                    "System secp script code hash error! found: {}, expected: {}",
                                    code_hash,
                                    SECP_CODE_HASH,
                                );
                            }
                            secp_code_hash = Some(code_hash);
                        }
                        CellOutPoint {
                            tx_hash: tx.hash().clone(),
                            index: index as u32,
                        }
                    })
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();

        secp_code_hash
            .map(|secp_code_hash| GenesisInfo {
                header,
                out_points,
                secp_code_hash,
            })
            .ok_or_else(|| "No code hash(secp) found in txs[0][1]".to_owned())
    }

    pub fn header(&self) -> &Header {
        &self.header
    }

    pub fn secp_code_hash(&self) -> &H256 {
        &self.secp_code_hash
    }

    pub fn secp_dep(&self) -> OutPoint {
        OutPoint {
            cell: Some(self.out_points[0][1].clone()),
            block_hash: None,
        }
    }
}

#[derive(Debug)]
pub struct TransferTransactionBuilder<'a> {
    pub from_address: &'a Address,
    pub from_capacity: u64,
    pub to_data: &'a Bytes,
    pub to_address: &'a Address,
    pub to_capacity: u64,
}

impl<'a> TransferTransactionBuilder<'a> {
    pub fn build<F>(
        &self,
        input_infos: Vec<LiveCellInfo>,
        genesis_info: &GenesisInfo,
        build_witness: F,
    ) -> Result<RpcTransaction, String>
    where
        F: FnOnce(&H256) -> Result<Bytes, String>,
    {
        assert!(self.from_capacity >= self.to_capacity);
        let secp_dep = genesis_info.secp_dep();
        let secp_code_hash = genesis_info.secp_code_hash();

        let inputs = input_infos
            .iter()
            .map(LiveCellInfo::core_input)
            .collect::<Vec<_>>();

        // TODO: calculate transaction fee
        // Send to user
        let mut from_capacity = self.from_capacity;
        let mut outputs = vec![CellOutput {
            capacity: Capacity::shannons(self.to_capacity),
            data: self.to_data.clone(),
            lock: self.to_address.lock_script(secp_code_hash.clone()),
            type_: None,
        }];
        from_capacity -= self.to_capacity;

        if from_capacity > MIN_SECP_CELL_CAPACITY {
            // The rest send back to sender
            outputs.push(CellOutput {
                capacity: Capacity::shannons(from_capacity),
                data: Bytes::default(),
                lock: self.from_address.lock_script(secp_code_hash.clone()),
                type_: None,
            });
        }

        let core_tx = TransactionBuilder::default()
            .inputs(inputs.clone())
            .outputs(outputs.clone())
            .dep(secp_dep.clone())
            .build();

        let witness = vec![build_witness(core_tx.hash())?];
        let witnesses = inputs.iter().map(|_| witness.clone()).collect::<Vec<_>>();
        Ok((&TransactionBuilder::default()
            .inputs(inputs)
            .outputs(outputs)
            .dep(secp_dep)
            .witnesses(witnesses)
            .build())
            .into())
    }
}

pub fn build_witness_with_key(privkey: &secp256k1::SecretKey, tx_hash: &H256) -> Bytes {
    let message = secp256k1::Message::from_slice(&blake2b_256(tx_hash))
        .expect("Convert to secp256k1 message failed");
    serialize_signature(&SECP256K1.sign_recoverable(&message, privkey))
}

pub fn serialize_signature(signature: &secp256k1::RecoverableSignature) -> Bytes {
    let (recov_id, data) = signature.serialize_compact();
    let mut signature_bytes = [0u8; 65];
    signature_bytes[0..64].copy_from_slice(&data[0..64]);
    signature_bytes[64] = recov_id.to_i32() as u8;
    Bytes::from(signature_bytes.to_vec())
}
