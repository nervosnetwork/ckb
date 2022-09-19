use crate::{
    pool::Pool,
    store::{Batch, IteratorDirection, Store},
};

use crate::error::Error;
use ckb_types::{
    core::{BlockNumber, BlockView},
    packed::{Byte32, Bytes, CellOutput, OutPoint, Script},
    prelude::*,
};

use std::convert::TryInto;
use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
};

pub type TxIndex = u32;
pub type OutputIndex = u32;
pub type CellIndex = u32;
pub enum CellType {
    Input,
    Output,
}

/// +--------------+--------------------+--------------------------+
/// | KeyPrefix::  | Key::              | Value::                  |
/// +--------------+--------------------+--------------------------+
/// | 0            | OutPoint           | Cell                     |
/// | 32           | ConsumedOutPoint   | Cell                     | * rollback and prune
/// | 64           | CellLockScript     | TxHash                   |
/// | 96           | CellTypeScript     | TxHash                   |
/// | 128          | TxLockScript       | TxHash                   |
/// | 160          | TxTypeScript       | TxHash                   |
/// | 192          | TxHash             | TransactionInputs        | * rollback and prune
/// | 224          | Header             | Transactions             |
/// +--------------+--------------------+--------------------------+

pub enum Key<'a> {
    OutPoint(&'a OutPoint),
    ConsumedOutPoint(BlockNumber, &'a OutPoint),
    CellLockScript(&'a Script, BlockNumber, TxIndex, OutputIndex),
    CellTypeScript(&'a Script, BlockNumber, TxIndex, OutputIndex),
    TxLockScript(&'a Script, BlockNumber, TxIndex, CellIndex, CellType),
    TxTypeScript(&'a Script, BlockNumber, TxIndex, CellIndex, CellType),
    TxHash(&'a Byte32),
    Header(BlockNumber, &'a Byte32),
}

pub enum Value<'a> {
    Cell(BlockNumber, TxIndex, &'a CellOutput, &'a Bytes),
    TxHash(&'a Byte32),
    TransactionInputs(Vec<OutPoint>),
    Transactions(Vec<(Byte32, u32)>),
}

#[repr(u8)]
pub enum KeyPrefix {
    OutPoint = 0,
    ConsumedOutPoint = 32,
    CellLockScript = 64,
    CellTypeScript = 96,
    TxLockScript = 128,
    TxTypeScript = 160,
    TxHash = 192,
    Header = 224,
}

impl<'a> Key<'a> {
    pub fn into_vec(self) -> Vec<u8> {
        self.into()
    }
}

impl<'a> From<Key<'a>> for Vec<u8> {
    fn from(key: Key<'a>) -> Vec<u8> {
        let mut encoded = Vec::new();

        match key {
            Key::OutPoint(out_point) => {
                encoded.push(KeyPrefix::OutPoint as u8);
                encoded.extend_from_slice(out_point.as_slice());
            }
            Key::ConsumedOutPoint(block_number, out_point) => {
                encoded.push(KeyPrefix::ConsumedOutPoint as u8);
                encoded.extend_from_slice(&block_number.to_be_bytes());
                encoded.extend_from_slice(out_point.as_slice());
            }
            Key::CellLockScript(script, block_number, tx_index, output_index) => {
                encoded.push(KeyPrefix::CellLockScript as u8);
                append_key(&mut encoded, script, block_number, tx_index, output_index);
            }
            Key::CellTypeScript(script, block_number, tx_index, output_index) => {
                encoded.push(KeyPrefix::CellTypeScript as u8);
                append_key(&mut encoded, script, block_number, tx_index, output_index);
            }
            Key::TxLockScript(script, block_number, tx_index, io_index, io_type) => {
                encoded.push(KeyPrefix::TxLockScript as u8);
                append_key(&mut encoded, script, block_number, tx_index, io_index);
                match io_type {
                    CellType::Input => encoded.push(0),
                    CellType::Output => encoded.push(1),
                }
            }
            Key::TxTypeScript(script, block_number, tx_index, io_index, io_type) => {
                encoded.push(KeyPrefix::TxTypeScript as u8);
                append_key(&mut encoded, script, block_number, tx_index, io_index);
                match io_type {
                    CellType::Input => encoded.push(0),
                    CellType::Output => encoded.push(1),
                }
            }
            Key::TxHash(tx_hash) => {
                encoded.push(KeyPrefix::TxHash as u8);
                encoded.extend_from_slice(tx_hash.as_slice());
            }
            Key::Header(block_number, block_hash) => {
                encoded.push(KeyPrefix::Header as u8);
                encoded.extend_from_slice(&block_number.to_be_bytes());
                encoded.extend_from_slice(block_hash.as_slice());
            }
        }
        encoded
    }
}

fn append_key(
    encoded: &mut Vec<u8>,
    script: &Script,
    block_number: u64,
    tx_index: u32,
    io_index: u32,
) {
    encoded.extend_from_slice(&extract_raw_data(script));
    encoded.extend_from_slice(&block_number.to_be_bytes());
    encoded.extend_from_slice(&tx_index.to_be_bytes());
    encoded.extend_from_slice(&io_index.to_be_bytes());
}

// a helper fn extracts script fields raw data
pub fn extract_raw_data(script: &Script) -> Vec<u8> {
    [
        script.code_hash().as_slice(),
        script.hash_type().as_slice(),
        &script.args().raw_data(),
    ]
    .concat()
}

impl<'a> From<Value<'a>> for Vec<u8> {
    fn from(value: Value<'a>) -> Vec<u8> {
        let mut encoded = Vec::new();
        match value {
            Value::Cell(block_number, tx_index, output, output_data) => {
                encoded.extend_from_slice(&block_number.to_le_bytes());
                encoded.extend_from_slice(&tx_index.to_le_bytes());
                encoded.extend_from_slice(output.as_slice());
                encoded.extend_from_slice(output_data.as_slice());
            }
            Value::TxHash(tx_hash) => {
                encoded.extend_from_slice(tx_hash.as_slice());
            }
            Value::TransactionInputs(out_points) => {
                out_points
                    .iter()
                    .for_each(|out_point| encoded.extend_from_slice(out_point.as_slice()));
            }
            Value::Transactions(txs) => {
                txs.iter().for_each(|(tx_hash, outputs_len)| {
                    encoded.extend_from_slice(tx_hash.as_slice());
                    encoded.extend_from_slice(&(outputs_len).to_le_bytes());
                });
            }
        }
        encoded
    }
}

impl<'a> Value<'a> {
    pub fn parse_cell_value(slice: &[u8]) -> (BlockNumber, TxIndex, CellOutput, Bytes) {
        let block_number =
            BlockNumber::from_le_bytes(slice[0..8].try_into().expect("stored cell block_number"));
        let tx_index =
            TxIndex::from_le_bytes(slice[8..12].try_into().expect("stored cell tx_index"));
        let output_size =
            u32::from_le_bytes(slice[12..16].try_into().expect("stored cell output_size")) as usize;
        let output =
            CellOutput::from_slice(&slice[12..12 + output_size]).expect("stored cell output");
        let output_data =
            Bytes::from_slice(&slice[12 + output_size..]).expect("stored cell output_data");
        (block_number, tx_index, output, output_data)
    }

    pub fn parse_transactions_value(slice: &[u8]) -> Vec<(Byte32, u32)> {
        slice
            .chunks_exact(36) // hash(32) + outputs_len(4)
            .map(|s| {
                let tx_hash = Byte32::from_slice(&s[0..32]).expect("stored block value: tx_hash");
                let outputs_len = u32::from_le_bytes(
                    s[32..].try_into().expect("stored block value: outputs_len"),
                );
                (tx_hash, outputs_len)
            })
            .collect()
    }
}

pub struct DetailedLiveCell {
    pub block_number: BlockNumber,
    pub block_hash: Byte32,
    pub tx_index: TxIndex,
    pub cell_output: CellOutput,
    pub cell_data: Bytes,
}

#[derive(Clone)]
pub struct Indexer<S> {
    store: S,
    // number of blocks to keep for rollback and forking, for example:
    // keep_num: 100, current tip: 321, will prune ConsumedOutPoint / TxHash kv pair which block_number <= 221
    keep_num: u64,
    prune_interval: u64,
    // an optional overlay to index the pending txs in the ckb tx pool
    // currently only supports removals of dead cells from the pending txs
    pool: Option<Arc<RwLock<Pool>>>,
}

impl<S> Indexer<S> {
    pub fn new(
        store: S,
        keep_num: u64,
        prune_interval: u64,
        pool: Option<Arc<RwLock<Pool>>>,
    ) -> Self {
        Self {
            store,
            keep_num,
            prune_interval,
            pool,
        }
    }

    pub fn store(&self) -> &S {
        &self.store
    }
}

impl<S> Indexer<S>
where
    S: Store,
{
    pub fn append(&self, block: &BlockView) -> Result<(), Error> {
        let mut batch = self.store.batch()?;
        // insert block transactions
        batch.put_kv(
            Key::Header(block.number(), &block.hash()),
            Value::Transactions(
                block
                    .transactions()
                    .iter()
                    .map(|tx| (tx.hash(), tx.outputs().len() as u32))
                    .collect(),
            ),
        )?;

        let block_number = block.number();
        let transactions = block.transactions();
        let pool = self.pool.as_ref().map(|p| p.write().expect("acquire lock"));
        for (tx_index, tx) in transactions.iter().enumerate() {
            let tx_index = tx_index as u32;
            let tx_hash = tx.hash();
            // skip cellbase
            if tx_index > 0 {
                for (input_index, input) in tx.inputs().into_iter().enumerate() {
                    // delete live cells related kv and mark it as consumed (for rollback and forking)
                    // insert lock / type => tx_hash mapping
                    let input_index = input_index as u32;
                    let out_point = input.previous_output();
                    let key_vec = Key::OutPoint(&out_point).into_vec();

                    let stored_live_cell = self
                        .store
                        .get(&key_vec)?
                        .or_else(|| {
                            transactions
                                .iter()
                                .enumerate()
                                .find(|(_i, tx)| tx.hash() == out_point.tx_hash())
                                .map(|(i, tx)| {
                                    Value::Cell(
                                        block_number,
                                        i as u32,
                                        &tx.outputs()
                                            .get(out_point.index().unpack())
                                            .expect("index should match"),
                                        &tx.outputs_data()
                                            .get(out_point.index().unpack())
                                            .expect("index should match"),
                                    )
                                    .into()
                                })
                        })
                        .expect("stored live cell or consume output in same block");

                    let (generated_by_block_number, generated_by_tx_index, output, _output_data) =
                        Value::parse_cell_value(&stored_live_cell);

                    batch.delete(
                        Key::CellLockScript(
                            &output.lock(),
                            generated_by_block_number,
                            generated_by_tx_index,
                            out_point.index().unpack(),
                        )
                        .into_vec(),
                    )?;
                    batch.put_kv(
                        Key::TxLockScript(
                            &output.lock(),
                            block_number,
                            tx_index,
                            input_index,
                            CellType::Input,
                        ),
                        Value::TxHash(&tx_hash),
                    )?;
                    if let Some(script) = output.type_().to_opt() {
                        batch.delete(
                            Key::CellTypeScript(
                                &script,
                                generated_by_block_number,
                                generated_by_tx_index,
                                out_point.index().unpack(),
                            )
                            .into_vec(),
                        )?;
                        batch.put_kv(
                            Key::TxTypeScript(
                                &script,
                                block_number,
                                tx_index,
                                input_index,
                                CellType::Input,
                            ),
                            Value::TxHash(&tx_hash),
                        )?;
                    };
                    batch.delete(key_vec)?;
                    batch.put_kv(
                        Key::ConsumedOutPoint(block_number, &out_point),
                        stored_live_cell,
                    )?;
                }
            }

            for (output_index, output) in tx.outputs().into_iter().enumerate() {
                // insert live cells related kv
                // insert lock / type => tx_hash mapping
                let output_data = tx
                    .outputs_data()
                    .get(output_index)
                    .expect("outputs_data len should equals outputs len");
                let output_index = output_index as u32;
                let out_point = OutPoint::new(tx.hash(), output_index);
                batch.put_kv(
                    Key::CellLockScript(&output.lock(), block_number, tx_index, output_index),
                    Value::TxHash(&tx_hash),
                )?;
                batch.put_kv(
                    Key::TxLockScript(
                        &output.lock(),
                        block_number,
                        tx_index,
                        output_index,
                        CellType::Output,
                    ),
                    Value::TxHash(&tx_hash),
                )?;
                if let Some(script) = output.type_().to_opt() {
                    batch.put_kv(
                        Key::CellTypeScript(&script, block_number, tx_index, output_index),
                        Value::TxHash(&tx_hash),
                    )?;
                    batch.put_kv(
                        Key::TxTypeScript(
                            &script,
                            block_number,
                            tx_index,
                            output_index,
                            CellType::Output,
                        ),
                        Value::TxHash(&tx_hash),
                    )?;
                }
                batch.put_kv(
                    Key::OutPoint(&out_point),
                    Value::Cell(block_number, tx_index, &output, &output_data),
                )?;
            }

            // insert tx
            batch.put_kv(
                Key::TxHash(&tx_hash),
                Value::TransactionInputs(
                    tx.inputs()
                        .into_iter()
                        .map(|input| input.previous_output())
                        .collect(),
                ),
            )?;
        }

        batch.commit()?;

        if let Some(mut pool) = pool {
            pool.transactions_commited(&transactions);
        }

        if block_number % self.prune_interval == 0 {
            self.prune()?;
        }
        Ok(())
    }

    pub fn rollback(&self) -> Result<(), Error> {
        if let Some((block_number, block_hash)) = self.tip()? {
            let mut batch = self.store.batch()?;
            let txs = Value::parse_transactions_value(
                &self
                    .store
                    .get(Key::Header(block_number, &block_hash).into_vec())?
                    .expect("stored block"),
            );
            for (tx_index, (tx_hash, outputs_len)) in txs.into_iter().enumerate().rev() {
                let tx_index = tx_index as u32;
                // rollback live cells
                for output_index in 0..outputs_len {
                    let out_point = OutPoint::new(tx_hash.clone(), output_index);
                    let out_point_key = Key::OutPoint(&out_point).into_vec();

                    let (_generated_by_block_number, _generated_by_tx_index, output, _output_data) =
                        if let Some(stored_live_cell) = self.store.get(&out_point_key)? {
                            Value::parse_cell_value(&stored_live_cell)
                        } else {
                            let consumed_cell = self
                                .store
                                .get(Key::ConsumedOutPoint(block_number, &out_point).into_vec())?
                                .expect("stored live cell or consume output in same block");
                            Value::parse_cell_value(&consumed_cell)
                        };

                    batch.delete(
                        Key::CellLockScript(&output.lock(), block_number, tx_index, output_index)
                            .into_vec(),
                    )?;
                    batch.delete(
                        Key::TxLockScript(
                            &output.lock(),
                            block_number,
                            tx_index,
                            output_index,
                            CellType::Output,
                        )
                        .into_vec(),
                    )?;
                    if let Some(script) = output.type_().to_opt() {
                        batch.delete(
                            Key::CellTypeScript(&script, block_number, tx_index, output_index)
                                .into_vec(),
                        )?;
                        batch.delete(
                            Key::TxTypeScript(
                                &script,
                                block_number,
                                tx_index,
                                output_index,
                                CellType::Output,
                            )
                            .into_vec(),
                        )?;
                    };
                    batch.delete(out_point_key)?;
                }

                // rollback inputs
                let transaction_key = Key::TxHash(&tx_hash).into_vec();
                // skip cellbase
                if tx_index > 0 {
                    for (input_index, out_point) in self
                        .store
                        .get(&transaction_key)?
                        .expect("stored transaction inputs")
                        .chunks_exact(OutPoint::TOTAL_SIZE)
                        .map(|slice| {
                            OutPoint::from_slice(slice)
                                .expect("stored transaction inputs out_point slice")
                        })
                        .enumerate()
                    {
                        let input_index = input_index as u32;
                        let consumed_out_point_key =
                            Key::ConsumedOutPoint(block_number, &out_point).into_vec();

                        let stored_consumed_cell = self
                            .store
                            .get(consumed_out_point_key)?
                            .expect("stored consumed cells value");
                        let (
                            generated_by_block_number,
                            generated_by_tx_index,
                            output,
                            _output_data,
                        ) = Value::parse_cell_value(&stored_consumed_cell);

                        batch.put_kv(
                            Key::CellLockScript(
                                &output.lock(),
                                generated_by_block_number,
                                generated_by_tx_index,
                                out_point.index().unpack(),
                            ),
                            Value::TxHash(&out_point.tx_hash()),
                        )?;
                        batch.delete(
                            Key::TxLockScript(
                                &output.lock(),
                                block_number,
                                tx_index,
                                input_index,
                                CellType::Input,
                            )
                            .into_vec(),
                        )?;
                        if let Some(script) = output.type_().to_opt() {
                            batch.put_kv(
                                Key::CellTypeScript(
                                    &script,
                                    generated_by_block_number,
                                    generated_by_tx_index,
                                    out_point.index().unpack(),
                                ),
                                Value::TxHash(&out_point.tx_hash()),
                            )?;
                            batch.delete(
                                Key::TxTypeScript(
                                    &script,
                                    block_number,
                                    tx_index,
                                    input_index,
                                    CellType::Input,
                                )
                                .into_vec(),
                            )?;
                        }
                        batch.put_kv(Key::OutPoint(&out_point), stored_consumed_cell)?;
                    }
                }
                // delete transaction
                batch.delete(transaction_key)?;
            }

            // delete block transactions
            batch.delete(Key::Header(block_number, &block_hash).into_vec())?;

            batch.commit()?;
        }
        Ok(())
    }

    pub fn tip(&self) -> Result<Option<(BlockNumber, Byte32)>, Error> {
        let mut iter = self
            .store
            .iter(&[KeyPrefix::Header as u8 + 1], IteratorDirection::Reverse)?;
        Ok(iter.next().map(|(key, _)| {
            (
                BlockNumber::from_be_bytes(key[1..9].try_into().expect("stored block key")),
                Byte32::from_slice(&key[9..]).expect("stored block key"),
            )
        }))
    }

    pub fn get_block_hash(&self, block_number: BlockNumber) -> Result<Option<Byte32>, Error> {
        let mut key_prefix_header = vec![KeyPrefix::Header as u8];
        key_prefix_header.extend_from_slice(&block_number.to_be_bytes());
        Ok(
            match self
                .store
                .iter(&key_prefix_header, IteratorDirection::Forward)?
                .next()
            {
                Some((key, _v)) if key.starts_with(&key_prefix_header) => {
                    Some(Byte32::from_slice(&key[9..]).expect("stored block key"))
                }
                _ => None,
            },
        )
    }

    pub fn prune(&self) -> Result<(), Error> {
        let (tip_number, _tip_hash) = self.tip()?.expect("stored tip");
        if tip_number > self.keep_num {
            let prune_to_block = tip_number - self.keep_num;
            let mut batch = self.store.batch()?;
            // prune ConsumedOutPoint => Cell
            let key_prefix_consumed_out_point = vec![KeyPrefix::ConsumedOutPoint as u8];
            let iter = self
                .store
                .iter(&key_prefix_consumed_out_point, IteratorDirection::Forward)?
                .take_while(|(key, _value)| key.starts_with(&key_prefix_consumed_out_point));
            for (_block_number, key) in iter
                .map(|(key, _value)| {
                    (
                        BlockNumber::from_be_bytes(
                            key[1..9].try_into().expect("stored block_number"),
                        ),
                        key,
                    )
                })
                .take_while(|(block_number, _key)| prune_to_block.gt(block_number))
            {
                batch.delete(key)?;
            }

            // prune TxHash => TransactionInputs
            let mut key_prefix_header = vec![KeyPrefix::Header as u8];
            key_prefix_header.extend_from_slice(&prune_to_block.to_be_bytes());
            let iter = self
                .store
                .iter(&key_prefix_header, IteratorDirection::Reverse)?
                .take_while(|(key, _value)| key.starts_with(&[KeyPrefix::Header as u8]));
            for txs in iter.map(|(_key, value)| Value::parse_transactions_value(&value)) {
                let (first_tx_hash, _) = txs.get(0).expect("none empty block");
                if self.store.exists(Key::TxHash(first_tx_hash).into_vec())? {
                    for (tx_hash, _outputs_len) in txs {
                        batch.delete(Key::TxHash(&tx_hash).into_vec())?;
                    }
                } else {
                    break;
                }
            }

            batch.commit()?;
        }
        Ok(())
    }

    pub fn get_live_cells_by_lock_script(
        &self,
        lock_script: &Script,
    ) -> Result<Vec<OutPoint>, Error> {
        self.get_live_cells_by_script(lock_script, KeyPrefix::CellLockScript)
    }

    pub fn get_live_cells_by_type_script(
        &self,
        type_script: &Script,
    ) -> Result<Vec<OutPoint>, Error> {
        self.get_live_cells_by_script(type_script, KeyPrefix::CellTypeScript)
    }

    fn get_live_cells_by_script(
        &self,
        script: &Script,
        prefix: KeyPrefix,
    ) -> Result<Vec<OutPoint>, Error> {
        let mut start_key = vec![prefix as u8];
        start_key.extend_from_slice(&extract_raw_data(script));

        let iter = self.store.iter(&start_key, IteratorDirection::Forward)?;
        Ok(iter
            .take_while(|(key, _)| key.starts_with(&start_key))
            .map(|(key, value)| {
                let tx_hash = Byte32::from_slice(&value).expect("stored tx hash");
                let index = OutputIndex::from_be_bytes(
                    key[key.len() - 4..].try_into().expect("stored index"),
                );
                OutPoint::new(tx_hash, index)
            })
            .collect())
    }

    pub fn get_transactions_by_lock_script(
        &self,
        lock_script: &Script,
    ) -> Result<Vec<Byte32>, Error> {
        self.get_transactions_by_script(lock_script, KeyPrefix::TxLockScript)
    }

    pub fn get_transactions_by_type_script(
        &self,
        type_script: &Script,
    ) -> Result<Vec<Byte32>, Error> {
        self.get_transactions_by_script(type_script, KeyPrefix::TxTypeScript)
    }

    fn get_transactions_by_script(
        &self,
        script: &Script,
        prefix: KeyPrefix,
    ) -> Result<Vec<Byte32>, Error> {
        let mut start_key = vec![prefix as u8];
        start_key.extend_from_slice(&extract_raw_data(script));

        let iter = self.store.iter(&start_key, IteratorDirection::Forward)?;
        Ok(iter
            .take_while(|(key, _)| key.starts_with(&start_key))
            .map(|(_key, value)| Byte32::from_slice(&value).expect("stored tx hash"))
            .collect())
    }

    /// Given an OutPoint representing a live cell, returns the following components
    /// related to the live cell:
    /// * CellOutput
    /// * Cell data
    /// * Block hash in which the cell is created
    pub fn get_detailed_live_cell(
        &self,
        out_point: &OutPoint,
    ) -> Result<Option<DetailedLiveCell>, Error> {
        let key_vec = Key::OutPoint(out_point).into_vec();
        let (block_number, tx_index, cell_output, cell_data) = match self.store.get(&key_vec)? {
            Some(stored_cell) => Value::parse_cell_value(&stored_cell),
            None => return Ok(None),
        };
        let mut header_start_key = vec![KeyPrefix::Header as u8];
        header_start_key.extend_from_slice(&block_number.to_be_bytes());
        let mut iter = self
            .store
            .iter(&header_start_key, IteratorDirection::Forward)?;
        let block_hash = match iter.next() {
            Some((key, _)) => {
                if key.starts_with(&header_start_key) {
                    let start = std::mem::size_of::<BlockNumber>() + 1;
                    Byte32::from_slice(&key[start..start + 32]).expect("stored key header hash")
                } else {
                    return Ok(None);
                }
            }
            None => return Ok(None),
        };
        Ok(Some(DetailedLiveCell {
            block_number,
            block_hash,
            tx_index,
            cell_output,
            cell_data,
        }))
    }

    pub fn report(&self) -> Result<(), Error> {
        let iter = self.store.iter(&[], IteratorDirection::Forward)?;
        let mut statistics: HashMap<u8, (usize, usize, usize)> = HashMap::new();
        for (key, value) in iter {
            let s = statistics.entry(*key.first().unwrap()).or_default();
            s.0 += 1;
            s.1 += key.len();
            s.2 += value.len();
        }
        println!("{:?}", statistics);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::RocksdbStore;
    use ckb_types::{
        bytes::Bytes,
        core::{
            capacity_bytes, BlockBuilder, Capacity, HeaderBuilder, ScriptHashType,
            TransactionBuilder,
        },
        packed::{CellInput, CellOutputBuilder, OutPoint, ScriptBuilder},
        H256,
    };
    use tempfile;

    fn new_indexer<S: Store>(prefix: &str) -> Indexer<S> {
        let tmp_dir = tempfile::Builder::new().prefix(prefix).tempdir().unwrap();
        let store = S::new(tmp_dir.path().to_str().unwrap());
        Indexer::new(store, 10, 1, None)
    }

    #[test]
    fn append_and_rollback_to_empty() {
        let indexer = new_indexer::<RocksdbStore>("append_and_rollback_to_empty");

        let lock_script1 = ScriptBuilder::default()
            .code_hash(H256(rand::random()).pack())
            .hash_type(ScriptHashType::Data.into())
            .args(Bytes::from(b"lock_script1".to_vec()).pack())
            .build();

        let lock_script2 = ScriptBuilder::default()
            .code_hash(H256(rand::random()).pack())
            .hash_type(ScriptHashType::Type.into())
            .args(Bytes::from(b"lock_script2".to_vec()).pack())
            .build();

        let type_script1 = ScriptBuilder::default()
            .code_hash(H256(rand::random()).pack())
            .hash_type(ScriptHashType::Data.into())
            .args(Bytes::from(b"type_script1".to_vec()).pack())
            .build();

        let type_script2 = ScriptBuilder::default()
            .code_hash(H256(rand::random()).pack())
            .hash_type(ScriptHashType::Type.into())
            .args(Bytes::from(b"type_script2".to_vec()).pack())
            .build();

        let cellbase0 = TransactionBuilder::default()
            .input(CellInput::new_cellbase_input(0))
            .witness(Script::default().into_witness())
            .output(
                CellOutputBuilder::default()
                    .capacity(capacity_bytes!(1000).pack())
                    .lock(lock_script1.clone())
                    .build(),
            )
            .output_data(Default::default())
            .build();

        let tx00 = TransactionBuilder::default()
            .output(
                CellOutputBuilder::default()
                    .capacity(capacity_bytes!(1000).pack())
                    .lock(lock_script1.clone())
                    .type_(Some(type_script1.clone()).pack())
                    .build(),
            )
            .output_data(Default::default())
            .build();

        let tx01 = TransactionBuilder::default()
            .output(
                CellOutputBuilder::default()
                    .capacity(capacity_bytes!(2000).pack())
                    .lock(lock_script2.clone())
                    .type_(Some(type_script2.clone()).pack())
                    .build(),
            )
            .output_data(Default::default())
            .build();

        let block0 = BlockBuilder::default()
            .transaction(cellbase0)
            .transaction(tx00)
            .transaction(tx01)
            .header(HeaderBuilder::default().number(0.pack()).build())
            .build();

        indexer.append(&block0).unwrap();

        let (tip_number, tip_hash) = indexer.tip().unwrap().unwrap();
        assert_eq!(0, tip_number);
        assert_eq!(block0.hash(), tip_hash);

        indexer.rollback().unwrap();

        // tip should be None and store should be empty;
        assert!(indexer.tip().unwrap().is_none());
        let mut iter = indexer.store.iter(&[], IteratorDirection::Forward).unwrap();
        assert!(iter.next().is_none());
    }

    #[test]
    fn append_two_blocks_and_rollback_one() {
        let indexer = new_indexer::<RocksdbStore>("append_two_blocks_and_rollback_one");

        let lock_script1 = ScriptBuilder::default()
            .code_hash(H256(rand::random()).pack())
            .hash_type(ScriptHashType::Data.into())
            .args(Bytes::from(b"lock_script1".to_vec()).pack())
            .build();

        let lock_script2 = ScriptBuilder::default()
            .code_hash(H256(rand::random()).pack())
            .hash_type(ScriptHashType::Type.into())
            .args(Bytes::from(b"lock_script2".to_vec()).pack())
            .build();

        let type_script1 = ScriptBuilder::default()
            .code_hash(H256(rand::random()).pack())
            .hash_type(ScriptHashType::Data.into())
            .args(Bytes::from(b"type_script1".to_vec()).pack())
            .build();

        let type_script2 = ScriptBuilder::default()
            .code_hash(H256(rand::random()).pack())
            .hash_type(ScriptHashType::Type.into())
            .args(Bytes::from(b"type_script2".to_vec()).pack())
            .build();

        let cellbase0 = TransactionBuilder::default()
            .input(CellInput::new_cellbase_input(0))
            .witness(Script::default().into_witness())
            .output(
                CellOutputBuilder::default()
                    .capacity(capacity_bytes!(1000).pack())
                    .lock(lock_script1.clone())
                    .build(),
            )
            .output_data(Default::default())
            .build();

        let tx00 = TransactionBuilder::default()
            .output(
                CellOutputBuilder::default()
                    .capacity(capacity_bytes!(1000).pack())
                    .lock(lock_script1.clone())
                    .type_(Some(type_script1.clone()).pack())
                    .build(),
            )
            .output_data(Default::default())
            .build();

        let tx01 = TransactionBuilder::default()
            .output(
                CellOutputBuilder::default()
                    .capacity(capacity_bytes!(2000).pack())
                    .lock(lock_script2.clone())
                    .type_(Some(type_script2.clone()).pack())
                    .build(),
            )
            .output_data(Default::default())
            .build();

        let block0 = BlockBuilder::default()
            .transaction(cellbase0)
            .transaction(tx00.clone())
            .transaction(tx01.clone())
            .header(HeaderBuilder::default().number(0.pack()).build())
            .build();
        indexer.append(&block0).unwrap();

        let cellbase1 = TransactionBuilder::default()
            .input(CellInput::new_cellbase_input(1))
            .witness(Script::default().into_witness())
            .output(
                CellOutputBuilder::default()
                    .capacity(capacity_bytes!(1000).pack())
                    .lock(lock_script1.clone())
                    .build(),
            )
            .output_data(Default::default())
            .build();

        let tx10 = TransactionBuilder::default()
            .input(CellInput::new(OutPoint::new(tx00.hash(), 0), 0))
            .output(
                CellOutputBuilder::default()
                    .capacity(capacity_bytes!(1000).pack())
                    .lock(lock_script1.clone())
                    .type_(Some(type_script1.clone()).pack())
                    .build(),
            )
            .output_data(Default::default())
            .build();

        let tx11 = TransactionBuilder::default()
            .input(CellInput::new(OutPoint::new(tx01.hash(), 0), 0))
            .output(
                CellOutputBuilder::default()
                    .capacity(capacity_bytes!(2000).pack())
                    .lock(lock_script2.clone())
                    .type_(Some(type_script2.clone()).pack())
                    .build(),
            )
            .output_data(Default::default())
            .build();

        let block1 = BlockBuilder::default()
            .transaction(cellbase1)
            .transaction(tx10.clone())
            .transaction(tx11.clone())
            .header(
                HeaderBuilder::default()
                    .number(1.pack())
                    .parent_hash(block0.hash())
                    .build(),
            )
            .build();

        indexer.append(&block1).unwrap();
        let (tip_number, tip_hash) = indexer.tip().unwrap().unwrap();
        assert_eq!(1, tip_number);
        assert_eq!(block1.hash(), tip_hash);
        assert_eq!(
            3, // cellbase0, cellbase1, tx10
            indexer
                .get_live_cells_by_lock_script(&lock_script1)
                .unwrap()
                .len()
        );
        assert_eq!(
            5, //cellbase0, cellbase1, tx00 (output), tx10(input and output)
            indexer
                .get_transactions_by_lock_script(&lock_script1)
                .unwrap()
                .len()
        );

        indexer.rollback().unwrap();
        let (tip_number, tip_hash) = indexer.tip().unwrap().unwrap();
        assert_eq!(0, tip_number);
        assert_eq!(block0.hash(), tip_hash);
        assert_eq!(
            2, //cellbase0, tx00
            indexer
                .get_live_cells_by_lock_script(&lock_script1)
                .unwrap()
                .len()
        );
        assert_eq!(
            2, //cellbase0, tx00 (output)
            indexer
                .get_transactions_by_lock_script(&lock_script1)
                .unwrap()
                .len()
        );
    }

    #[test]
    fn consume_output_in_same_block() {
        let indexer = new_indexer::<RocksdbStore>("consume_output_in_same_block");

        let lock_script1 = ScriptBuilder::default()
            .code_hash(H256(rand::random()).pack())
            .hash_type(ScriptHashType::Data.into())
            .args(Bytes::from(b"lock_script1".to_vec()).pack())
            .build();

        let lock_script2 = ScriptBuilder::default()
            .code_hash(H256(rand::random()).pack())
            .hash_type(ScriptHashType::Type.into())
            .args(Bytes::from(b"lock_script2".to_vec()).pack())
            .build();

        let type_script1 = ScriptBuilder::default()
            .code_hash(H256(rand::random()).pack())
            .hash_type(ScriptHashType::Data.into())
            .args(Bytes::from(b"type_script1".to_vec()).pack())
            .build();

        let type_script2 = ScriptBuilder::default()
            .code_hash(H256(rand::random()).pack())
            .hash_type(ScriptHashType::Type.into())
            .args(Bytes::from(b"type_script2".to_vec()).pack())
            .build();

        let cellbase0 = TransactionBuilder::default()
            .input(CellInput::new_cellbase_input(0))
            .witness(Script::default().into_witness())
            .output(
                CellOutputBuilder::default()
                    .capacity(capacity_bytes!(1000).pack())
                    .lock(lock_script1.clone())
                    .build(),
            )
            .output_data(Default::default())
            .build();

        let tx00 = TransactionBuilder::default()
            .output(
                CellOutputBuilder::default()
                    .capacity(capacity_bytes!(1000).pack())
                    .lock(lock_script1.clone())
                    .type_(Some(type_script1.clone()).pack())
                    .build(),
            )
            .output_data(Default::default())
            .build();

        let tx01 = TransactionBuilder::default()
            .output(
                CellOutputBuilder::default()
                    .capacity(capacity_bytes!(2000).pack())
                    .lock(lock_script2.clone())
                    .type_(Some(type_script2.clone()).pack())
                    .build(),
            )
            .output_data(Default::default())
            .build();

        let block0 = BlockBuilder::default()
            .transaction(cellbase0.clone())
            .transaction(tx00.clone())
            .transaction(tx01.clone())
            .header(HeaderBuilder::default().number(0.pack()).build())
            .build();
        indexer.append(&block0).unwrap();

        let cellbase1 = TransactionBuilder::default()
            .input(CellInput::new_cellbase_input(1))
            .witness(Script::default().into_witness())
            .output(
                CellOutputBuilder::default()
                    .capacity(capacity_bytes!(1000).pack())
                    .lock(lock_script1.clone())
                    .build(),
            )
            .output_data(Default::default())
            .build();

        let tx10 = TransactionBuilder::default()
            .input(CellInput::new(OutPoint::new(tx00.hash(), 0), 0))
            .output(
                CellOutputBuilder::default()
                    .capacity(capacity_bytes!(1000).pack())
                    .lock(lock_script1.clone())
                    .type_(Some(type_script1.clone()).pack())
                    .build(),
            )
            .output_data(Default::default())
            .build();

        let tx11 = TransactionBuilder::default()
            .input(CellInput::new(OutPoint::new(tx01.hash(), 0), 0))
            .output(
                CellOutputBuilder::default()
                    .capacity(capacity_bytes!(2000).pack())
                    .lock(lock_script2.clone())
                    .type_(Some(type_script2.clone()).pack())
                    .build(),
            )
            .output_data(Default::default())
            .build();

        let tx12 = TransactionBuilder::default()
            .input(CellInput::new(OutPoint::new(tx11.hash(), 0), 0))
            .output(
                CellOutputBuilder::default()
                    .capacity(capacity_bytes!(2000).pack())
                    .lock(lock_script2.clone())
                    .type_(Some(type_script2.clone()).pack())
                    .build(),
            )
            .output_data(Default::default())
            .build();

        let block1 = BlockBuilder::default()
            .transaction(cellbase1)
            .transaction(tx10.clone())
            .transaction(tx11.clone())
            .transaction(tx12.clone())
            .header(
                HeaderBuilder::default()
                    .number(1.pack())
                    .parent_hash(block0.hash())
                    .build(),
            )
            .build();

        indexer.append(&block1).unwrap();
        assert_eq!(
            1, // tx12
            indexer
                .get_live_cells_by_lock_script(&lock_script2)
                .unwrap()
                .len()
        );
        assert_eq!(
            5, // tx01 (output), tx11 (input / output), tx12 (input / output)
            indexer
                .get_transactions_by_lock_script(&lock_script2)
                .unwrap()
                .len()
        );

        indexer.rollback().unwrap();
        let live_cells = indexer
            .get_live_cells_by_lock_script(&lock_script1)
            .unwrap();
        //cellbase0, tx00
        assert_eq!(
            vec![
                OutPoint::new(cellbase0.hash(), 0),
                OutPoint::new(tx00.hash(), 0)
            ],
            live_cells
        );

        let transactions = indexer
            .get_transactions_by_lock_script(&lock_script1)
            .unwrap();

        //cellbase0, tx00
        assert_eq!(vec![cellbase0.hash(), tx00.hash(),], transactions);
    }

    #[test]
    fn prune() {
        let indexer = new_indexer::<RocksdbStore>("prune");

        let lock_script1 = ScriptBuilder::default()
            .code_hash(H256(rand::random()).pack())
            .hash_type(ScriptHashType::Data.into())
            .args(Bytes::from(b"lock_script1".to_vec()).pack())
            .build();

        let lock_script2 = ScriptBuilder::default()
            .code_hash(H256(rand::random()).pack())
            .hash_type(ScriptHashType::Type.into())
            .args(Bytes::from(b"lock_script2".to_vec()).pack())
            .build();

        let type_script1 = ScriptBuilder::default()
            .code_hash(H256(rand::random()).pack())
            .hash_type(ScriptHashType::Data.into())
            .args(Bytes::from(b"type_script1".to_vec()).pack())
            .build();

        let type_script2 = ScriptBuilder::default()
            .code_hash(H256(rand::random()).pack())
            .hash_type(ScriptHashType::Type.into())
            .args(Bytes::from(b"type_script2".to_vec()).pack())
            .build();

        let cellbase0 = TransactionBuilder::default()
            .input(CellInput::new_cellbase_input(0))
            .witness(Script::default().into_witness())
            .output(
                CellOutputBuilder::default()
                    .capacity(capacity_bytes!(1000).pack())
                    .lock(lock_script1.clone())
                    .build(),
            )
            .output_data(Default::default())
            .build();

        let tx00 = TransactionBuilder::default()
            .output(
                CellOutputBuilder::default()
                    .capacity(capacity_bytes!(1000).pack())
                    .lock(lock_script1.clone())
                    .type_(Some(type_script1.clone()).pack())
                    .build(),
            )
            .output_data(Default::default())
            .build();

        let tx01 = TransactionBuilder::default()
            .output(
                CellOutputBuilder::default()
                    .capacity(capacity_bytes!(2000).pack())
                    .lock(lock_script2.clone())
                    .type_(Some(type_script2.clone()).pack())
                    .build(),
            )
            .output_data(Default::default())
            .build();

        let block0 = BlockBuilder::default()
            .transaction(cellbase0)
            .transaction(tx00.clone())
            .transaction(tx01.clone())
            .header(HeaderBuilder::default().number(0.pack()).build())
            .build();

        indexer.append(&block0).unwrap();

        let (mut pre_tx0, mut pre_tx1, mut pre_block) = (tx00, tx01, block0);

        for i in 0..20 {
            let cellbase = TransactionBuilder::default()
                .input(CellInput::new_cellbase_input(i + 1))
                .witness(Script::default().into_witness())
                .output(
                    CellOutputBuilder::default()
                        .capacity(capacity_bytes!(1000).pack())
                        .lock(lock_script1.clone())
                        .build(),
                )
                .output_data(Default::default())
                .build();

            pre_tx0 = TransactionBuilder::default()
                .input(CellInput::new(OutPoint::new(pre_tx0.hash(), 0), 0))
                .output(
                    CellOutputBuilder::default()
                        .capacity(capacity_bytes!(1000).pack())
                        .lock(lock_script1.clone())
                        .type_(Some(type_script1.clone()).pack())
                        .build(),
                )
                .output_data(Default::default())
                .build();

            pre_tx1 = TransactionBuilder::default()
                .input(CellInput::new(OutPoint::new(pre_tx1.hash(), 0), 0))
                .output(
                    CellOutputBuilder::default()
                        .capacity(capacity_bytes!(2000).pack())
                        .lock(lock_script2.clone())
                        .type_(Some(type_script2.clone()).pack())
                        .build(),
                )
                .output_data(Default::default())
                .build();

            pre_block = BlockBuilder::default()
                .transaction(cellbase)
                .transaction(pre_tx0.clone())
                .transaction(pre_tx1.clone())
                .header(
                    HeaderBuilder::default()
                        .number((pre_block.number() + 1).pack())
                        .parent_hash(pre_block.hash())
                        .build(),
                )
                .build();

            indexer.append(&pre_block).unwrap();
        }

        let key_prefix = [KeyPrefix::ConsumedOutPoint as u8];
        let stored_consumed_out_points: Vec<_> = indexer
            .store
            .iter(&key_prefix, IteratorDirection::Forward)
            .unwrap()
            .take_while(|(key, _value)| key.starts_with(&key_prefix))
            .map(|(key, _value)| key)
            .collect();
        assert_eq!(22, stored_consumed_out_points.len());

        let key_prefix = [KeyPrefix::TxHash as u8];
        let stored_tx_hashes: Vec<_> = indexer
            .store
            .iter(&key_prefix, IteratorDirection::Forward)
            .unwrap()
            .take_while(|(key, _value)| key.starts_with(&key_prefix))
            .map(|(key, _value)| key)
            .collect();
        // 11 blocks, 3 txs per block
        assert_eq!(33, stored_tx_hashes.len());
    }

    #[test]
    fn append_and_rollback_with_cellbase() {
        let indexer = new_indexer::<RocksdbStore>("append_and_rollback_with_cellbase");

        let lock_script1 = ScriptBuilder::default()
            .code_hash(H256(rand::random()).pack())
            .hash_type(ScriptHashType::Data.into())
            .args(Bytes::from(b"lock_script1".to_vec()).pack())
            .build();

        let lock_script2 = ScriptBuilder::default()
            .code_hash(H256(rand::random()).pack())
            .hash_type(ScriptHashType::Type.into())
            .args(Bytes::from(b"lock_script2".to_vec()).pack())
            .build();

        let type_script1 = ScriptBuilder::default()
            .code_hash(H256(rand::random()).pack())
            .hash_type(ScriptHashType::Data.into())
            .args(Bytes::from(b"type_script1".to_vec()).pack())
            .build();

        let type_script2 = ScriptBuilder::default()
            .code_hash(H256(rand::random()).pack())
            .hash_type(ScriptHashType::Type.into())
            .args(Bytes::from(b"type_script2".to_vec()).pack())
            .build();

        let cellbase0 = TransactionBuilder::default()
            .input(CellInput::new_cellbase_input(0))
            .witness(Script::default().into_witness())
            .output(
                CellOutputBuilder::default()
                    .capacity(capacity_bytes!(1000).pack())
                    .lock(lock_script1.clone())
                    .build(),
            )
            .output_data(Default::default())
            .build();

        let tx00 = TransactionBuilder::default()
            .output(
                CellOutputBuilder::default()
                    .capacity(capacity_bytes!(1000).pack())
                    .lock(lock_script1.clone())
                    .type_(Some(type_script1.clone()).pack())
                    .build(),
            )
            .output_data(Default::default())
            .build();

        let tx01 = TransactionBuilder::default()
            .output(
                CellOutputBuilder::default()
                    .capacity(capacity_bytes!(2000).pack())
                    .lock(lock_script2.clone())
                    .type_(Some(type_script2.clone()).pack())
                    .build(),
            )
            .output_data(Default::default())
            .build();

        let block0 = BlockBuilder::default()
            .transaction(cellbase0)
            .transaction(tx00.clone())
            .transaction(tx01.clone())
            .header(HeaderBuilder::default().number(0.pack()).build())
            .build();

        indexer.append(&block0).unwrap();

        let (tip_number, tip_hash) = indexer.tip().unwrap().unwrap();
        assert_eq!(0, tip_number);
        assert_eq!(block0.hash(), tip_hash);

        indexer.rollback().unwrap();

        // tip should be None and store should be empty;
        assert!(indexer.tip().unwrap().is_none());
        let mut iter = indexer.store.iter(&[], IteratorDirection::Forward).unwrap();
        assert!(iter.next().is_none());
    }

    // test bug fix of https://github.com/quake/ckb-indexer/issues/7
    #[test]
    fn prune_should_not_delete_live_cells() {
        let indexer = new_indexer::<RocksdbStore>("prune_should_not_delete_live_cells");

        let all_zero_lock_script = ScriptBuilder::default()
            .code_hash(H256([0; 32]).pack())
            .hash_type(ScriptHashType::Data.into())
            .build();

        let lock_script1 = ScriptBuilder::default()
            .code_hash(H256(rand::random()).pack())
            .hash_type(ScriptHashType::Data.into())
            .args(Bytes::from(b"lock_script1".to_vec()).pack())
            .build();

        let cellbase0 = TransactionBuilder::default()
            .input(CellInput::new_cellbase_input(0))
            .witness(Script::default().into_witness())
            .output(
                CellOutputBuilder::default()
                    .capacity(capacity_bytes!(1000).pack())
                    .lock(all_zero_lock_script.clone())
                    .build(),
            )
            .output_data(Default::default())
            .build();

        let block0 = BlockBuilder::default()
            .transaction(cellbase0)
            .header(HeaderBuilder::default().number(0.pack()).build())
            .build();

        indexer.append(&block0).unwrap();

        assert_eq!(
            1, //cellbase0
            indexer
                .get_live_cells_by_lock_script(&all_zero_lock_script)
                .unwrap()
                .len()
        );

        let mut pre_block = block0;

        // keep_num is 10, use 11 to trigger prune
        for i in 0..11 {
            let cellbase = TransactionBuilder::default()
                .input(CellInput::new_cellbase_input(i + 1))
                .witness(Script::default().into_witness())
                .output(
                    CellOutputBuilder::default()
                        .capacity(capacity_bytes!(1000).pack())
                        .lock(lock_script1.clone())
                        .build(),
                )
                .output_data(Default::default())
                .build();

            pre_block = BlockBuilder::default()
                .transaction(cellbase)
                .header(
                    HeaderBuilder::default()
                        .number((pre_block.number() + 1).pack())
                        .parent_hash(pre_block.hash())
                        .build(),
                )
                .build();

            indexer.append(&pre_block).unwrap();
        }

        // should not delete live cells by mistake
        assert_eq!(
            1, //cellbase0
            indexer
                .get_live_cells_by_lock_script(&all_zero_lock_script)
                .unwrap()
                .len()
        );
    }

    #[test]
    fn get_block_hash() {
        let indexer = new_indexer::<RocksdbStore>("get_block_hash");

        let block_hashes: Vec<Byte32> = (0..10)
            .map(|i| {
                let cellbase = TransactionBuilder::default()
                    .input(CellInput::new_cellbase_input(i))
                    .build();
                let block = BlockBuilder::default()
                    .transaction(cellbase)
                    .header(HeaderBuilder::default().number(i.pack()).build())
                    .build();
                indexer.append(&block).unwrap();
                block.hash()
            })
            .collect();

        block_hashes.into_iter().enumerate().for_each(|(i, hash)| {
            assert_eq!(
                hash,
                indexer.get_block_hash(i as BlockNumber).unwrap().unwrap()
            )
        });

        assert!(indexer.get_block_hash(10).unwrap().is_none());
    }

    #[test]
    fn rollback_block_should_update_lock_script_and_type_script_index_correctly() {
        let indexer = new_indexer::<RocksdbStore>(
            "rollback_block_should_update_lock_script_and_type_script_index_correctly",
        );

        let lock_script1 = ScriptBuilder::default()
            .code_hash(H256(rand::random()).pack())
            .hash_type(ScriptHashType::Data.into())
            .args(Bytes::from(b"lock_script1".to_vec()).pack())
            .build();

        let type_script1 = ScriptBuilder::default()
            .code_hash(H256(rand::random()).pack())
            .hash_type(ScriptHashType::Data.into())
            .args(Bytes::from(b"type_script1".to_vec()).pack())
            .build();

        let cellbase0 = TransactionBuilder::default()
            .input(CellInput::new_cellbase_input(0))
            .witness(Script::default().into_witness())
            .output(
                CellOutputBuilder::default()
                    .capacity(capacity_bytes!(1000).pack())
                    .lock(lock_script1.clone())
                    .build(),
            )
            .output_data(Default::default())
            .build();

        let tx00 = TransactionBuilder::default()
            .output(
                CellOutputBuilder::default()
                    .capacity(capacity_bytes!(1000).pack())
                    .lock(lock_script1.clone())
                    .type_(Some(type_script1.clone()).pack())
                    .build(),
            )
            .output_data(Default::default())
            .build();

        let block0 = BlockBuilder::default()
            .transaction(cellbase0.clone())
            .transaction(tx00.clone())
            .header(HeaderBuilder::default().number(0.pack()).build())
            .build();
        indexer.append(&block0).unwrap();

        let cellbase1 = TransactionBuilder::default()
            .input(CellInput::new_cellbase_input(1))
            .witness(Script::default().into_witness())
            .output(
                CellOutputBuilder::default()
                    .capacity(capacity_bytes!(1000).pack())
                    .lock(lock_script1.clone())
                    .build(),
            )
            .output_data(Default::default())
            .build();

        let tx10 = TransactionBuilder::default()
            .input(CellInput::new(OutPoint::new(tx00.hash(), 0), 0))
            .output(
                CellOutputBuilder::default()
                    .capacity(capacity_bytes!(1000).pack())
                    .lock(lock_script1.clone())
                    .type_(Some(type_script1.clone()).pack())
                    .build(),
            )
            .output_data(Default::default())
            .build();

        let block1 = BlockBuilder::default()
            .transaction(cellbase1)
            .transaction(tx10.clone())
            .header(
                HeaderBuilder::default()
                    .number(1.pack())
                    .parent_hash(block0.hash())
                    .build(),
            )
            .build();

        indexer.append(&block1).unwrap();
        assert_eq!(
            3, // cellbase0, cellbase1, tx10
            indexer
                .get_live_cells_by_lock_script(&lock_script1)
                .unwrap()
                .len()
        );
        assert_eq!(
            5, //cellbase0, cellbase1, tx00 (output), tx10(input and output)
            indexer
                .get_transactions_by_lock_script(&lock_script1)
                .unwrap()
                .len()
        );

        indexer.rollback().unwrap();

        let live_cells = indexer
            .get_live_cells_by_lock_script(&lock_script1)
            .unwrap();
        //cellbase0, tx00
        assert_eq!(
            vec![
                OutPoint::new(cellbase0.hash(), 0),
                OutPoint::new(tx00.hash(), 0)
            ],
            live_cells
        );

        let live_cells = indexer
            .get_live_cells_by_type_script(&type_script1)
            .unwrap();
        //tx00 (output)
        assert_eq!(vec![OutPoint::new(tx00.hash(), 0)], live_cells);

        let transactions = indexer
            .get_transactions_by_lock_script(&lock_script1)
            .unwrap();
        //cellbase0, tx00
        assert_eq!(vec![cellbase0.hash(), tx00.hash()], transactions);

        let transactions = indexer
            .get_transactions_by_type_script(&type_script1)
            .unwrap();
        //tx00 (output)
        assert_eq!(vec![tx00.hash()], transactions);
    }
}
