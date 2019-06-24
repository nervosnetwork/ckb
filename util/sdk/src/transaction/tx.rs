use std::collections::HashMap;

use ckb_core::{
    cell::{
        resolve_transaction, BlockInfo, CellMeta, CellMetaBuilder, CellProvider, CellStatus,
        HeaderProvider, HeaderStatus,
    },
    extras::BlockExt,
    header::Header,
    transaction::{CellOutPoint, CellOutput, OutPoint, Transaction, TransactionBuilder, Witness},
    Cycle,
};
use ckb_script::{DataLoader, ScriptConfig, TransactionScriptsVerifier};
use fnv::FnvHashSet;
use numext_fixed_hash::{H160, H256};
use rocksdb::{ColumnFamily, IteratorMode, Options, DB};
use serde_derive::{Deserialize, Serialize};

use super::{from_local_cell_out_point, CellAliasManager, CellManager};
use crate::wallet::{KeyStore, KeyStoreError};
use crate::{HttpRpcClient, ROCKSDB_COL_TX};

pub struct TransactionManager<'a> {
    cf: ColumnFamily<'a>,
    db: &'a DB,
}

impl<'a> TransactionManager<'a> {
    pub fn new(db: &'a DB) -> TransactionManager {
        let cf = db.cf_handle(ROCKSDB_COL_TX).unwrap_or_else(|| {
            db.create_cf(ROCKSDB_COL_TX, &Options::default())
                .unwrap_or_else(|_| panic!("Create ColumnFamily {} failed", ROCKSDB_COL_TX))
        });
        TransactionManager { cf, db }
    }

    pub fn add(&self, tx: &Transaction) -> Result<(), String> {
        if tx.inputs().len() != tx.witnesses().len() {
            return Err(format!(
                "Invalid witnesses length: {}, expected: {}",
                tx.witnesses().len(),
                tx.inputs().len(),
            ));
        }
        // TODO: check all deps can be found
        // TODO: check all inputs can be found
        // TODO: check all output can be found
        let key_bytes = tx.hash().to_vec();
        let value_bytes = bincode::serialize(tx).unwrap();
        self.db.put_cf(self.cf, key_bytes, value_bytes)?;
        Ok(())
    }

    pub fn set_witness(
        &self,
        hash: &H256,
        input_index: usize,
        witness: Witness,
    ) -> Result<Transaction, String> {
        let tx = self.get(hash)?;
        if input_index >= tx.inputs().len() {
            return Err("input index out of bound".to_owned());
        }
        let mut witnesses = tx.witnesses().to_vec();
        witnesses[input_index] = witness;
        let tx_new = TransactionBuilder::from_transaction(tx)
            .witnesses_clear()
            .witnesses(witnesses)
            .build();
        assert_eq!(
            hash,
            tx_new.hash(),
            "Transaction hash must not changed just update witness"
        );
        self.add(&tx_new)?;
        Ok(tx_new)
    }

    pub fn set_witnesses_by_keys(
        &self,
        hash: &H256,
        key_store: &mut KeyStore,
        rpc_client: &mut HttpRpcClient,
        secp_code_hash: &H256,
    ) -> Result<Transaction, String> {
        let tx = self.get(hash)?;
        let tx_hash = tx.hash();
        let mut witnesses = tx.witnesses().to_vec();
        let cell_alias_manager = CellAliasManager::new(self.db);
        let cell_manager = CellManager::new(self.db);
        for (idx, input) in tx.inputs().iter().enumerate() {
            let cell_out_point = input.previous_output.cell.as_ref().unwrap();
            let cell_output = from_local_cell_out_point(cell_out_point)
                .or_else(|_| cell_alias_manager.get(&cell_out_point))
                .and_then(|name| cell_manager.get(&name))
                .or_else(|_| {
                    let out_point = OutPoint {
                        cell: Some(cell_out_point.clone()),
                        block_hash: None,
                    };
                    rpc_client
                        .get_live_cell(out_point.into())
                        .call()
                        .unwrap()
                        .cell
                        .map(Into::into)
                        .ok_or_else(|| {
                            format!(
                                "Input(tx-hash: {:#x}, index: {}) not found or dead",
                                cell_out_point.tx_hash, cell_out_point.index,
                            )
                        })
                })?;

            let lock = cell_output.lock;
            if &lock.code_hash == secp_code_hash {
                if let Some(lock_arg) = lock
                    .args
                    .get(0)
                    .and_then(|bytes| H160::from_slice(bytes).ok())
                {
                    let signature = key_store.sign_recoverable(&lock_arg, tx_hash)
                        .map_err(|err| {
                            match err {
                                KeyStoreError::AccountLocked(lock_arg) => {
                                    format!("Account(lock_arg={:x}) locked or not exists, your may use `account unlock` to unlock it", lock_arg)
                                }
                                err => err.to_string(),
                            }
                        })?;
                    let (recov_id, data) = signature.serialize_compact();
                    let mut signature_bytes = [0u8; 65];
                    signature_bytes[0..64].copy_from_slice(&data[0..64]);
                    signature_bytes[64] = recov_id.to_i32() as u8;
                    log::debug!("set witness[{}] by pubkey hash(lock_arg): {:x}", idx, hash);
                    witnesses[idx] = vec![signature_bytes.to_vec().into()];
                } else {
                    log::warn!("Can not find key for secp arg: {:?}", lock.args.get(0));
                }
            } else {
                log::info!("Input with a non-secp lock: code_hash={}", lock.code_hash);
            }
        }
        let new_tx = TransactionBuilder::from_transaction(tx)
            .witnesses_clear()
            .witnesses(witnesses)
            .build();
        self.add(&new_tx)?;
        Ok(new_tx)
    }

    pub fn remove(&self, hash: &H256) -> Result<Transaction, String> {
        let tx = self.get(hash)?;
        self.db.delete_cf(self.cf, hash.as_bytes())?;
        Ok(tx)
    }

    pub fn get(&self, hash: &H256) -> Result<Transaction, String> {
        match self.db.get_cf(self.cf, hash.as_bytes())? {
            Some(db_vec) => Ok(bincode::deserialize(&db_vec).unwrap()),
            None => Err(format!("tx not found: {:#x}", hash)),
        }
    }

    pub fn list(&self) -> Result<Vec<Transaction>, String> {
        let mut txs = Vec::new();
        for (key_bytes, value_bytes) in self.db.iterator_cf(self.cf, IteratorMode::Start)? {
            let key = H256::from_slice(&key_bytes).unwrap();
            let tx: Transaction = bincode::deserialize(&value_bytes).unwrap();
            assert_eq!(
                &key,
                tx.hash(),
                "Transaction hash not match the transaction"
            );
            txs.push(tx);
        }
        Ok(txs)
    }

    pub fn verify(
        &self,
        hash: &H256,
        max_cycle: Cycle,
        rpc_client: &mut HttpRpcClient,
    ) -> Result<VerifyResult, String> {
        let tx = self.get(hash)?;
        let cell_manager = CellManager::new(self.db);
        let cell_alias_manager = CellAliasManager::new(self.db);
        let resource = Resource::from_both(&tx, &cell_manager, &cell_alias_manager, rpc_client)?;
        let rtx = {
            let mut seen_inputs = FnvHashSet::default();
            resolve_transaction(&tx, &mut seen_inputs, &resource, &resource)
                .map_err(|err| format!("Resolve transaction error: {:?}", err))?
        };

        let script_config = ScriptConfig::default();
        let verifier = TransactionScriptsVerifier::new(&rtx, &resource, &script_config);
        let cycle = verifier
            .verify(max_cycle)
            .map_err(|err| format!("Verify script error: {:?}", err))?;
        Ok(VerifyResult { cycle })
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct VerifyResult {
    pub cycle: Cycle,
    // debug_logs: Vec<String>,
}

struct Resource {
    out_point_blocks: HashMap<CellOutPoint, H256>,
    required_cells: HashMap<CellOutPoint, CellMeta>,
    required_headers: HashMap<H256, Header>,
}

impl Resource {
    fn from_both(
        tx: &Transaction,
        cell_manager: &CellManager,
        cell_alias_manager: &CellAliasManager,
        rpc_client: &mut HttpRpcClient,
    ) -> Result<Resource, String> {
        let mut out_point_blocks = HashMap::default();
        let mut required_headers = HashMap::default();
        let mut required_cells = HashMap::default();
        for out_point in tx
            .deps()
            .iter()
            .chain(tx.inputs().iter().map(|input| &input.previous_output))
        {
            let cell_out_point = out_point.cell.clone().unwrap();
            let mut block_info = None;
            if let Some(ref hash) = out_point.block_hash {
                let block_view = rpc_client
                    .get_block(hash.clone())
                    .call()
                    .unwrap()
                    .0
                    .unwrap();
                let header: Header = block_view.header.inner.into();
                block_info = Some(BlockInfo {
                    number: header.number(),
                    epoch: header.epoch(),
                });
                required_headers.insert(hash.clone(), header);
                out_point_blocks.insert(cell_out_point.clone(), hash.clone());
            }

            match cell_manager
                .get_by_cell_out_point(&cell_out_point)
                .or_else(|_| {
                    let name = cell_alias_manager.get(&cell_out_point)?;
                    cell_manager.get(&name)
                }) {
                Ok(cell_output) => {
                    let cell_meta =
                        cell_output_to_meta(cell_out_point.clone(), cell_output, block_info);
                    required_cells.insert(cell_out_point, cell_meta);
                }
                Err(_) => {
                    // TODO: we should cache genesis block here
                    let cell_output = rpc_client
                        .get_live_cell(out_point.clone().into())
                        .call()
                        .map_err(|err| {
                            format!("can not find out_point: {:?}, error={:?}", out_point, err)
                        })?
                        .cell
                        .unwrap()
                        .into();
                    let cell_meta =
                        cell_output_to_meta(cell_out_point.clone(), cell_output, block_info);
                    required_cells.insert(cell_out_point, cell_meta);
                }
            }
        }
        Ok(Resource {
            out_point_blocks,
            required_cells,
            required_headers,
        })
    }
}

fn cell_output_to_meta(
    cell_out_point: CellOutPoint,
    cell_output: CellOutput,
    block_info: Option<BlockInfo>,
) -> CellMeta {
    let data_hash = cell_output.data_hash();
    let mut cell_meta_builder = CellMetaBuilder::from_cell_output(cell_output)
        .out_point(cell_out_point.clone())
        .data_hash(data_hash);
    if let Some(block_info) = block_info {
        cell_meta_builder = cell_meta_builder.block_info(block_info);
    }
    cell_meta_builder.build()
}

impl<'a> HeaderProvider for Resource {
    fn header(&self, out_point: &OutPoint) -> HeaderStatus {
        out_point
            .block_hash
            .as_ref()
            .map(|block_hash| {
                if let Some(block_hash) = out_point.block_hash.as_ref() {
                    let cell_out_point = out_point.cell.as_ref().unwrap();
                    if let Some(saved_block_hash) = self.out_point_blocks.get(cell_out_point) {
                        if block_hash != saved_block_hash {
                            return HeaderStatus::InclusionFaliure;
                        }
                    }
                }
                self.required_headers
                    .get(block_hash)
                    .cloned()
                    .map(|header| {
                        // TODO: query index db ensure cell_out_point match the block_hash
                        HeaderStatus::live_header(header)
                    })
                    .unwrap_or(HeaderStatus::Unknown)
            })
            .unwrap_or(HeaderStatus::Unspecified)
    }
}

impl CellProvider for Resource {
    fn cell(&self, out_point: &OutPoint) -> CellStatus {
        self.required_cells
            .get(out_point.cell.as_ref().unwrap())
            .cloned()
            .map(CellStatus::live_cell)
            .unwrap_or(CellStatus::Unknown)
    }
}

impl DataLoader for Resource {
    // load CellOutput
    fn lazy_load_cell_output(&self, cell: &CellMeta) -> CellOutput {
        cell.cell_output.clone().unwrap_or_else(|| {
            self.required_cells
                .get(&cell.out_point)
                .and_then(|cell_meta| cell_meta.cell_output.clone())
                .unwrap()
        })
    }
    // load BlockExt
    fn get_block_ext(&self, _block_hash: &H256) -> Option<BlockExt> {
        // TODO: visit this later
        None
    }
}
