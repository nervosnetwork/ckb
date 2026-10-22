//! Produce independent real-secp transactions from a frozen canonical chain.
//!
//! Each input has a different signing group. The deterministic key chain is the
//! public local-dev fixture convention used by ckb-bench; it is not a wallet.

use super::super::{BenchResult, bench_error, finish_blake2b, require};
use ckb_crypto::secp::Privkey;
use ckb_types::{
    H256,
    bytes::Bytes,
    core::{BlockView, ScriptHashType, TransactionBuilder, TransactionView},
    packed::{CellDep, CellInput, CellOutput, OutPoint, Script, WitnessArgs},
    prelude::*,
};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    fs::File,
    io::{BufRead, BufReader, BufWriter, Write},
    path::Path,
};

const FEE_PER_INPUT: u64 = 10_000;
const MAX_TRANSACTIONS: usize = 65_536;

pub(super) struct Corpus {
    pub metadata: Value,
    pub transactions: Vec<TransactionView>,
}

fn digest(transactions: &[TransactionView]) -> String {
    let mut hash = ckb_hash::new_blake2b();
    for transaction in transactions {
        let data = transaction.data();
        hash.update(&(data.as_slice().len() as u64).to_le_bytes());
        hash.update(data.as_slice());
    }
    finish_blake2b(hash)
}

impl Corpus {
    pub fn read(path: &Path) -> BenchResult<Self> {
        let mut lines = BufReader::new(File::open(path)?).lines();
        let metadata: Value =
            serde_json::from_str(&lines.next().ok_or_else(|| bench_error("empty corpus"))??)?;
        require(
            metadata["schema"] == 1 && metadata["kind"] == "fixed_secp_corpus",
            "unknown corpus schema",
        )?;
        let count = metadata["count"]
            .as_u64()
            .ok_or_else(|| bench_error("missing corpus count"))?;
        require(
            (1..=MAX_TRANSACTIONS as u64).contains(&count),
            "corpus count out of bounds",
        )?;
        let inputs = metadata["inputs"]
            .as_u64()
            .ok_or_else(|| bench_error("missing corpus shape"))?;
        require(
            matches!(inputs, 1 | 2),
            "only 1in1out and 2in2out are supported",
        )?;
        let mut transactions = Vec::with_capacity(count as usize);
        let mut hashes = HashSet::new();
        let mut spent = HashSet::new();
        for line in lines {
            require(transactions.len() < count as usize, "too many corpus rows")?;
            let view: ckb_jsonrpc_types::TransactionView = serde_json::from_str(&line?)?;
            let declared_hash = view.hash.clone();
            let transaction = ckb_types::packed::Transaction::from(view.inner).into_view();
            require(
                H256::from(transaction.hash()) == declared_hash,
                "transaction hash differs from its bytes",
            )?;
            require(
                hashes.insert(transaction.hash()),
                "duplicate corpus transaction",
            )?;
            require(
                transaction.inputs().len() == inputs as usize
                    && transaction.outputs().len() == inputs as usize
                    && transaction.witnesses().len() == inputs as usize,
                "transaction differs from corpus shape",
            )?;
            for input in transaction.input_pts_iter() {
                require(
                    spent.insert(input),
                    "corpus spends one input more than once",
                )?;
            }
            transactions.push(transaction);
        }
        require(transactions.len() == count as usize, "incomplete corpus")?;
        require(
            metadata["transactions_blake2b"] == digest(&transactions),
            "corpus witness/byte digest mismatch",
        )?;
        require(
            transactions
                .iter()
                .flat_map(TransactionView::input_pts_iter)
                .all(|input| !hashes.contains(&input.tx_hash())),
            "external corpus must have independent transactions",
        )?;
        Ok(Self {
            metadata,
            transactions,
        })
    }
}

struct Owner {
    key: Privkey,
    cells: VecDeque<(OutPoint, CellOutput)>,
}

fn signed_transaction(
    cells: &[(OutPoint, CellOutput, &Privkey)],
    deps: &[CellDep],
) -> BenchResult<TransactionView> {
    let mut builder = TransactionBuilder::default().cell_deps(deps.iter().cloned());
    for (input, output, _) in cells {
        let capacity: u64 = output.capacity().unpack();
        let remaining = capacity
            .checked_sub(FEE_PER_INPUT)
            .ok_or_else(|| bench_error("funding cell cannot pay fee"))?;
        builder = builder
            .input(CellInput::new(input.clone(), 0))
            .output(output.clone().as_builder().capacity(remaining).build())
            .output_data(Bytes::new().pack());
    }
    let transaction = builder.build();
    let placeholder = WitnessArgs::new_builder()
        .lock(Some(Bytes::from(vec![0; 65])))
        .build();
    let witnesses = cells
        .iter()
        .map(|(_, _, key)| {
            let mut hash = ckb_hash::new_blake2b();
            hash.update(transaction.hash().as_slice());
            hash.update(&(placeholder.as_bytes().len() as u64).to_le_bytes());
            hash.update(&placeholder.as_bytes());
            let mut message = [0; 32];
            hash.finalize(&mut message);
            let signature: Bytes = key
                .sign_recoverable(&H256::from(message))?
                .serialize()
                .into();
            Ok(placeholder
                .clone()
                .as_builder()
                .lock(Some(signature))
                .build()
                .as_bytes()
                .pack())
        })
        .collect::<BenchResult<Vec<_>>>()?;
    Ok(transaction
        .as_advanced_builder()
        .set_witnesses(witnesses)
        .build())
}

pub(super) fn produce(
    funding: &Path,
    output: &Path,
    count: usize,
    inputs: usize,
) -> BenchResult<()> {
    require(
        (1..=MAX_TRANSACTIONS).contains(&count) && matches!(inputs, 1 | 2),
        "invalid corpus count or input count",
    )?;
    let mut source: Value = serde_json::from_reader(BufReader::new(File::open(funding)?))?;
    require(source["schema"] == 1, "unknown funding schema")?;
    let users = source["users"]
        .as_u64()
        .ok_or_else(|| bench_error("missing fixture users"))? as usize;
    require(
        (inputs..=10_000).contains(&users) && users.is_multiple_of(inputs),
        "fixture user count must be bounded and divisible by inputs",
    )?;
    let mut key: H256 = serde_json::from_value(source["seed_key"].clone())?;
    let code_hash: H256 = serde_json::from_value(source["lock_code_hash"].clone())?;
    let dep: CellDep =
        serde_json::from_value::<ckb_jsonrpc_types::CellDep>(source["cell_dep"].clone())?.into();
    let blocks: Vec<ckb_jsonrpc_types::BlockView> =
        serde_json::from_value(source["blocks"].take())?;
    require(!blocks.is_empty(), "funding has no genesis")?;
    let tip_number = blocks.len() - 1;
    let mut owners = Vec::with_capacity(users);
    let mut locks = HashMap::new();
    for index in 0..users {
        key = ckb_hash::blake2b_256(key.as_bytes()).into();
        let private_key: Privkey = key.clone().into();
        let args = ckb_hash::blake2b_256(private_key.pubkey()?.serialize());
        let lock = Script::new_builder()
            .code_hash(code_hash.clone())
            .hash_type(ScriptHashType::Type)
            .args(Bytes::copy_from_slice(&args[..20]))
            .build();
        require(
            locks.insert(lock, index).is_none(),
            "duplicate fixture signing key",
        )?;
        owners.push(Owner {
            key: private_key,
            cells: VecDeque::new(),
        });
    }
    let mut live = HashMap::new();
    let mut previous = None;
    let mut genesis = None;
    for (height, block) in blocks.into_iter().enumerate() {
        let declared = block.header.hash.clone();
        let block = BlockView::from(block);
        require(
            block.number() == height as u64 && H256::from(block.hash()) == declared,
            "funding chain height/hash mismatch",
        )?;
        if let Some(parent) = previous {
            require(
                block.parent_hash() == parent,
                "funding chain is not contiguous",
            )?;
        } else {
            genesis = Some(block.hash());
        }
        for transaction in block.transactions() {
            for input in transaction.input_pts_iter() {
                live.remove(&input);
            }
            for (index, (cell, data)) in transaction.outputs_with_data_iter().enumerate() {
                if data.is_empty() && cell.type_().is_none() && locks.contains_key(&cell.lock()) {
                    live.insert(OutPoint::new(transaction.hash(), index as u32), cell);
                }
            }
        }
        previous = Some(block.hash());
    }
    for (out_point, cell) in live {
        owners[locks[&cell.lock()]]
            .cells
            .push_back((out_point, cell));
    }
    for owner in &mut owners {
        owner
            .cells
            .make_contiguous()
            .sort_unstable_by(|a, b| a.0.as_slice().cmp(b.0.as_slice()));
    }
    let mut transactions = Vec::with_capacity(count);
    for index in 0..count {
        let first = index * inputs % users;
        let selected = owners[first..first + inputs]
            .iter_mut()
            .map(|owner| {
                let (point, cell) = owner
                    .cells
                    .pop_front()
                    .ok_or_else(|| bench_error("insufficient independent funding cells"))?;
                Ok((point, cell, &owner.key))
            })
            .collect::<BenchResult<Vec<_>>>()?;
        transactions.push(signed_transaction(&selected, std::slice::from_ref(&dep))?);
    }
    let metadata = json!({"schema":1,"kind":"fixed_secp_corpus","count":count,"inputs":inputs,
        "signing_groups":inputs,"fee_per_input":FEE_PER_INPUT,"genesis":genesis.map(H256::from),
        "tip_hash":previous.map(H256::from),"tip_number":tip_number,
        "transactions_blake2b":digest(&transactions)});
    // Never replace a frozen corpus, even when a caller accidentally reuses its path.
    let mut writer = BufWriter::new(File::options().write(true).create_new(true).open(output)?);
    serde_json::to_writer(&mut writer, &metadata)?;
    writeln!(writer)?;
    for transaction in transactions {
        serde_json::to_writer(
            &mut writer,
            &ckb_jsonrpc_types::TransactionView::from(transaction),
        )?;
        writeln!(writer)?;
    }
    writer.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn independent_signing_groups_pass_canonical_verification_and_reject_a_wrong_key()
    -> super::BenchResult<()> {
        use super::super::super::{bundled_cell, genesis_consensus, snapshot_with_genesis};
        use super::*;
        use ckb_script::{TransactionScriptsVerifier, TxVerifyEnv};
        use ckb_store::data_loader_wrapper::AsDataLoader;
        use ckb_types::core::cell::resolve_transaction;
        use std::sync::Arc;

        let (code, code_data) = bundled_cell("specs/cells/secp256k1_blake160_sighash_all");
        let (data, data_data) = bundled_cell("specs/cells/secp256k1_data");
        let code_hash = CellOutput::calc_data_hash(&code_data);
        let system = TransactionBuilder::default()
            .input(CellInput::new(OutPoint::null(), 0))
            .outputs([code, data])
            .outputs_data([code_data.pack(), data_data.pack()])
            .build();
        let keys: [Privkey; 2] = [H256::from([1; 32]).into(), H256::from([2; 32]).into()];
        let outputs = keys
            .iter()
            .map(|key| {
                let args = ckb_hash::blake2b_256(key.pubkey()?.serialize());
                Ok(CellOutput::new_builder()
                    .capacity(1000_u64 * 100_000_000)
                    .lock(
                        Script::new_builder()
                            .code_hash(code_hash.clone())
                            .hash_type(ScriptHashType::Data)
                            .args(Bytes::copy_from_slice(&args[..20]))
                            .build(),
                    )
                    .build())
            })
            .collect::<BenchResult<Vec<_>>>()?;
        let funding = TransactionBuilder::default()
            .input(CellInput::new(OutPoint::null(), 0))
            .outputs(outputs.clone())
            .outputs_data([Bytes::new().pack(), Bytes::new().pack()])
            .build();
        let deps: Vec<_> = (0..2)
            .map(|index| {
                CellDep::new_builder()
                    .out_point(OutPoint::new(system.hash(), index))
                    .build()
            })
            .collect();
        let consensus = Arc::new(genesis_consensus(vec![system, funding.clone()])?);
        let (_store, snapshot) = snapshot_with_genesis(Arc::clone(&consensus));
        let cells: Vec<_> = (0..2)
            .map(|index| {
                (
                    OutPoint::new(funding.hash(), index as u32),
                    outputs[index].clone(),
                    &keys[index],
                )
            })
            .collect();
        let verify = |transaction: TransactionView| -> BenchResult<u64> {
            let resolved = resolve_transaction(
                transaction,
                &mut HashSet::new(),
                snapshot.as_ref(),
                snapshot.as_ref(),
            )?;
            Ok(TransactionScriptsVerifier::new(
                Arc::new(resolved),
                snapshot.as_data_loader(),
                Arc::clone(&consensus),
                Arc::new(TxVerifyEnv::new_submit(snapshot.tip_header())),
            )
            .verify(consensus.max_block_cycles())?)
        };
        let one = signed_transaction(&cells[..1], &deps)?;
        let two = signed_transaction(&cells, &deps)?;
        let one_cycles = verify(one)?;
        assert!(one_cycles > 0 && verify(two.clone())? > one_cycles);
        let wrong = vec![
            (cells[0].0.clone(), cells[0].1.clone(), &keys[0]),
            (cells[1].0.clone(), cells[1].1.clone(), &keys[0]),
        ];
        assert!(verify(signed_transaction(&wrong, &deps)?).is_err());
        let corrupt = two
            .as_advanced_builder()
            .set_witnesses(vec![Bytes::from(vec![0; 85]).pack(); 2])
            .build();
        assert!(verify(corrupt).is_err());
        Ok(())
    }

    #[test]
    fn reader_rejects_changed_witnesses_double_spends_and_incomplete_corpora()
    -> super::BenchResult<()> {
        use super::super::super::build_tx;
        use super::*;
        let directory = tempfile::TempDir::new()?;
        let path = directory.path().join("corpus.jsonl");
        let transaction = build_tx(OutPoint::new(H256::from([1; 32]).into(), 0))
            .as_advanced_builder()
            .witness(Bytes::from(vec![1; 85]).pack())
            .build();
        let write = |transactions: &[TransactionView],
                     count: usize,
                     expected_digest: String|
         -> BenchResult<()> {
            let mut file = File::create(&path)?;
            writeln!(
                file,
                "{}",
                json!({"schema":1,"kind":"fixed_secp_corpus","count":count,"inputs":1,
                "transactions_blake2b":expected_digest})
            )?;
            for transaction in transactions {
                writeln!(
                    file,
                    "{}",
                    serde_json::to_string(&ckb_jsonrpc_types::TransactionView::from(
                        transaction.clone()
                    ))?
                )?;
            }
            Ok(())
        };
        let original = [transaction.clone()];
        write(&original, 1, digest(&original))?;
        assert!(Corpus::read(&path).is_ok());
        let corrupt = [transaction
            .as_advanced_builder()
            .set_witnesses(vec![Bytes::from(vec![2; 85]).pack()])
            .build()];
        write(&corrupt, 1, digest(&original))?;
        assert!(Corpus::read(&path).is_err());
        write(&original, 2, digest(&original))?;
        assert!(Corpus::read(&path).is_err());
        let duplicate_input = [
            transaction.clone(),
            transaction.as_advanced_builder().version(1_u32).build(),
        ];
        write(&duplicate_input, 2, digest(&duplicate_input))?;
        assert!(Corpus::read(&path).is_err());
        Ok(())
    }
}
