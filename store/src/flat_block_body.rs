use bincode::{self, Result};
use serde_derive::{Deserialize, Serialize};

use ckb_core::{
    transaction::{CellInput, CellOutput, OutPoint, Transaction, TransactionBuilder, Witness},
    Version,
};
use numext_fixed_hash::H256;

/// This module leverages bincode to build a new serializer with flat structure.
///
/// If we use bincode to serialize Vec<T>, it will create a series of bytes
/// which are essentially a black box for us. Even if we only need one of the
/// items within the Vec, we have to get the whole bytes, deserialize everything
/// and use the single item.
///
/// This flat serializer, on the other hand, will use bincode to serialize
/// each individual item separately, then it will simply concat all byte slices
/// to create a byte vector. With the generated Address indices in serialization,
/// we can then get a partial of all the data and deserialize individual item
/// separately.

const TRANSACTION_FIELDS_SIZE: usize = 9;

const TRANSACTION_TOTAL_SIZE_INDEX: usize = 0;
const TRANSACTION_OUTPUTS_INDEX: usize = 4;

type TransactionHeader = [usize; 9];

/// Address of a CellOutput.
#[derive(Clone, Serialize, Deserialize, Eq, PartialEq, Hash, Debug)]
pub(crate) struct CellOutputAddress {
    /// Offset in Block Body.
    pub offset: usize,
    /// Length.
    pub length: usize,
}

/// Address of a Transaction.
#[derive(Clone, Serialize, Deserialize, Eq, PartialEq, Hash, Debug)]
pub(crate) struct TransactionAddressInner {
    /// Index in Block.
    pub index: usize,
    /// Offset in Block Body.
    pub offset: usize,
    /// Length.
    pub length: usize,
    /// Offsets of inner fields.
    pub header: TransactionHeader,
    /// Offsets of CellOutputs.
    pub outputs_addresses: Vec<CellOutputAddress>,
}

/// Address of a Transaction in database.
#[derive(Clone, Serialize, Deserialize, Eq, PartialEq, Hash, Debug)]
pub(crate) struct TransactionAddressStored {
    pub inner: TransactionAddressInner,
    pub block_hash: H256,
}

impl TransactionAddressInner {
    pub(crate) fn into_stored(self, block_hash: H256) -> TransactionAddressStored {
        TransactionAddressStored {
            inner: self,
            block_hash,
        }
    }
}

fn serialized_transaction_size(
    tx: &Transaction,
) -> Result<(TransactionHeader, Vec<CellOutputAddress>)> {
    let config = bincode::config();
    let mut header = [0; TRANSACTION_FIELDS_SIZE];
    let size_header = config.serialized_size(&header)? as usize;
    let size_version = config.serialized_size(&tx.version())? as usize;
    let size_deps = config.serialized_size(tx.deps())? as usize;
    let size_inputs = config.serialized_size(tx.inputs())? as usize;
    let (size_outputs, output_addresses) = tx
        .outputs()
        .iter()
        .map(|output| config.serialized_size(output).map(|len| len as usize))
        .collect::<Result<Vec<usize>>>()?
        .into_iter()
        .fold(
            (0, Vec::with_capacity(tx.outputs().len())),
            |(offset, mut addresses), size| {
                addresses.push(CellOutputAddress {
                    offset,
                    length: size,
                });
                (offset + size, addresses)
            },
        );
    let size_witnesses = config.serialized_size(tx.witnesses())? as usize;
    let size_hash = config.serialized_size(tx.hash())? as usize;
    let size_witness_hash = config.serialized_size(tx.witness_hash())? as usize;
    for (idx, size) in [
        size_header,
        size_version,
        size_deps,
        size_inputs,
        size_outputs,
        size_witnesses,
        size_hash,
        size_witness_hash,
    ]
    .iter()
    .enumerate()
    {
        header[idx + 1] = header[idx] + size;
        header[TRANSACTION_TOTAL_SIZE_INDEX] += size;
    }
    let output_addresses = output_addresses
        .into_iter()
        .map(|mut addr| {
            addr.offset += header[TRANSACTION_OUTPUTS_INDEX];
            addr
        })
        .collect::<Vec<_>>();
    Ok((header, output_addresses))
}

pub(crate) fn deserialize_transaction(
    tx: &[u8],
    output_addresses: &[CellOutputAddress],
) -> Result<Transaction> {
    let config = bincode::config();
    let header: [usize; TRANSACTION_FIELDS_SIZE] = config.deserialize(tx)?;
    let version: Version = config.deserialize(&tx[header[1]..])?;
    let deps: Vec<OutPoint> = config.deserialize(&tx[header[2]..])?;
    let inputs: Vec<CellInput> = config.deserialize(&tx[header[3]..])?;
    let mut outputs = Vec::with_capacity(output_addresses.len());
    for addr in output_addresses.iter() {
        let output: CellOutput =
            config.deserialize(&tx[addr.offset..(addr.offset + addr.length)])?;
        outputs.push(output);
    }
    let witnesses: Vec<Witness> = config.deserialize(&tx[header[5]..])?;
    let hash: H256 = config.deserialize(&tx[header[6]..])?;
    let witness_hash: H256 = config.deserialize(&tx[header[7]..])?;
    Ok(TransactionBuilder::default()
        .version(version)
        .deps(deps)
        .inputs(inputs)
        .outputs(outputs)
        .witnesses(witnesses)
        .build_unchecked(hash, witness_hash))
}

pub(crate) fn serialize_block_body_size(
    txs: &[Transaction],
) -> Result<(usize, Vec<TransactionAddressInner>)> {
    let tx_indexes = txs
        .iter()
        .enumerate()
        .map(|(idx, tx)| {
            serialized_transaction_size(&tx)
                .map(|(header, outputs_addresses)| (idx, header, outputs_addresses))
        })
        .collect::<Result<Vec<(usize, TransactionHeader, Vec<CellOutputAddress>)>>>()?;
    let (block_size, tx_addresses) = tx_indexes.into_iter().fold(
        (0, Vec::with_capacity(txs.len())),
        |(offset, mut addresses), (index, header, outputs_addresses)| {
            addresses.push(TransactionAddressInner {
                index,
                offset,
                length: header[TRANSACTION_TOTAL_SIZE_INDEX],
                header,
                outputs_addresses,
            });
            (offset + header[TRANSACTION_TOTAL_SIZE_INDEX], addresses)
        },
    );
    Ok((block_size, tx_addresses))
}

pub(crate) fn serialize_block_body(
    txs: &[Transaction],
) -> Result<(Vec<u8>, Vec<TransactionAddressInner>)> {
    let (total_size, tx_addresses) = serialize_block_body_size(txs)?;
    let config = bincode::config();
    let mut bytes = Vec::with_capacity(total_size);
    for (idx, tx) in txs.iter().enumerate() {
        config.serialize_into(&mut bytes, &tx_addresses[idx].header)?;
        config.serialize_into(&mut bytes, &tx.version())?;
        config.serialize_into(&mut bytes, tx.deps())?;
        config.serialize_into(&mut bytes, tx.inputs())?;
        for output in tx.outputs().iter() {
            config.serialize_into(&mut bytes, output)?;
        }
        config.serialize_into(&mut bytes, tx.witnesses())?;
        config.serialize_into(&mut bytes, tx.hash())?;
        config.serialize_into(&mut bytes, tx.witness_hash())?;
    }
    Ok((bytes, tx_addresses))
}

pub(crate) fn deserialize_block_body(
    bytes: &[u8],
    tx_addresses: &[TransactionAddressInner],
) -> Vec<Transaction> {
    let txs = tx_addresses
        .iter()
        .map(|addr| {
            deserialize_transaction(
                &bytes[addr.offset..(addr.offset + addr.length)],
                &addr.outputs_addresses,
            )
        })
        .collect::<Result<Vec<Transaction>>>();
    txs.unwrap()
}
