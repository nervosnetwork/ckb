use ckb_types::core::{Capacity, TransactionBuilder, TransactionView};
use ckb_types::packed::{CellDep, CellInput, CellOutput, OutPoint, Script, ScriptOpt};
use ckb_types::prelude::*;
use rand::{thread_rng, Rng};
use std::collections::HashMap;

const EXPLODE_LIMIT: usize = 2000;

#[derive(Debug, Clone)]
pub struct TXO {
    out_point: OutPoint,
    output: CellOutput,
}

#[derive(Default, Debug)]
pub struct TXOSet {
    txos: HashMap<OutPoint, CellOutput>,
}

impl TXO {
    pub fn new(out_point: OutPoint, output: CellOutput) -> Self {
        Self { out_point, output }
    }

    pub fn out_point(&self) -> OutPoint {
        self.out_point.clone()
    }

    pub fn output(&self) -> CellOutput {
        self.output.clone()
    }

    pub fn capacity(&self) -> u64 {
        self.output.capacity().unpack()
    }

    pub fn lock(&self) -> Script {
        self.output.lock()
    }

    pub fn type_(&self) -> ScriptOpt {
        self.output.type_()
    }

    pub fn to_input(&self) -> CellInput {
        CellInput::new_builder()
            .previous_output(self.out_point.clone())
            .build()
    }

    /// Return a `CellOutput` with the equivalent capacity to the original TXO
    pub fn to_equivalent_output(&self) -> CellOutput {
        CellOutput::new_builder()
            .lock(self.lock())
            .capacity(self.capacity().pack())
            .build()
    }

    /// Return a `CellOutput` with the minimal capacity
    pub fn to_minimal_output(&self) -> CellOutput {
        CellOutput::new_builder()
            .lock(self.lock())
            .build_exact_capacity(Capacity::zero())
            .unwrap()
    }
}

impl TXOSet {
    pub fn new<T>(txos: T) -> Self
    where
        T: IntoIterator<Item = TXO>,
    {
        Self {
            txos: txos
                .into_iter()
                .map(|txo: TXO| (txo.out_point, txo.output))
                .collect(),
        }
    }

    pub fn len(&self) -> usize {
        self.txos.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn total_capacity(&self) -> u64 {
        self.iter().map(|txo| txo.capacity()).sum()
    }

    pub fn get(&self, out_point: &OutPoint) -> Option<TXO> {
        self.txos
            .get(out_point)
            .map(|output| TXO::new(out_point.clone(), output.clone()))
    }

    pub fn truncate(&mut self, len: usize) {
        if self.txos.len() > len {
            self.txos = self
                .txos
                .iter()
                .take(len)
                .map(|(out_point, output)| (out_point.clone(), output.clone()))
                .collect();
        }
    }

    pub fn extend<T>(&mut self, other: T)
    where
        T: Into<TXOSet>,
    {
        self.txos.extend(Into::<TXOSet>::into(other).txos)
    }

    /// Construct a transaction that explodes the large UTXOs into small UTXOs
    ///
    // NOTE: Limit the number of outputs, to avoid transaction size exceeding limit
    // NOTE: Assume empty output_data for simplicity
    pub fn boom<C>(&self, cell_deps: C) -> TransactionView
    where
        C: IntoIterator<Item = CellDep>,
    {
        let outputs = self.boom_to_minimals();
        let outputs_data: Vec<_> = outputs.iter().map(|_| Default::default()).collect();
        TransactionBuilder::default()
            .cell_deps(cell_deps)
            .inputs(self.iter().map(|txo| txo.to_input()))
            .outputs(outputs)
            .outputs_data(outputs_data)
            .build()
    }

    /// Construct transactions which convert the UTXO to another UTXO with equivalent capacity
    // NOTE: Assume empty output_data for simplicity
    pub fn bang<C>(&self, cell_deps: C) -> Vec<TransactionView>
    where
        C: IntoIterator<Item = CellDep>,
    {
        let cell_deps: Vec<_> = cell_deps.into_iter().collect();
        self.iter()
            .map(|txo| {
                TransactionBuilder::default()
                    .cell_deps(cell_deps.clone())
                    .input(txo.to_input())
                    .output(txo.to_equivalent_output())
                    .output_data(Default::default())
                    .build()
            })
            .collect()
    }

    /// Construct transactions which convert the UTXO to another UTXO, given random fees
    pub fn bang_random_fee<C>(&self, cell_deps: C) -> Vec<TransactionView>
    where
        C: IntoIterator<Item = CellDep>,
    {
        let cell_deps: Vec<_> = cell_deps.into_iter().collect();
        let mut rng = thread_rng();
        self.iter()
            .map(|txo| {
                let maximal_capacity = txo.capacity();
                let minimal_capacity: u64 = txo.to_minimal_output().capacity().unpack();
                let actual_capacity = rng.gen_range(minimal_capacity, maximal_capacity + 1);
                let output = txo
                    .to_equivalent_output()
                    .as_builder()
                    .capacity(actual_capacity.pack())
                    .build();
                TransactionBuilder::default()
                    .cell_deps(cell_deps.clone())
                    .input(txo.to_input())
                    .output(output)
                    .output_data(Default::default())
                    .build()
            })
            .collect()
    }

    fn boom_to_minimals(&self) -> Vec<CellOutput> {
        let mut input_capacity = self.total_capacity();
        let minimal = self.iter().next().unwrap().to_minimal_output();
        let minimal_capacity: u64 = minimal.capacity().unpack();
        let mut outputs = Vec::new();
        while outputs.len() < EXPLODE_LIMIT {
            if input_capacity < 2 * minimal_capacity || outputs.len() == EXPLODE_LIMIT {
                outputs.push(minimal.as_builder().capacity(input_capacity.pack()).build());
                break;
            } else {
                input_capacity -= minimal_capacity;
                outputs.push(minimal.clone());
            }
        }
        outputs
    }

    pub fn iter(&self) -> impl Iterator<Item = TXO> {
        // NOTE: Cloning is wasteful but okay for testing, I think
        self.txos
            .clone()
            .into_iter()
            .map(|(out_point, output)| TXO::new(out_point, output))
    }
}

impl From<&TransactionView> for TXOSet {
    fn from(transaction: &TransactionView) -> Self {
        let tx_hash = transaction.hash();
        let txos = transaction
            .outputs()
            .into_iter()
            .enumerate()
            .map(move |(i, output)| {
                let out_point = OutPoint::new_builder()
                    .tx_hash(tx_hash.clone())
                    .index(i.pack())
                    .build();
                TXO::new(out_point, output)
            });
        Self::new(txos)
    }
}
