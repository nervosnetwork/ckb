use ckb_core::transaction::{CellInput, CellOutput, Transaction};
use ckb_protocol::{
    CellInput as FbsCellInput, CellInputBuilder, CellOutput as FbsCellOutput, CellOutputBuilder,
    OutPoint as FbsOutPoint, Transaction as FbsTransaction, TransactionBuilder,
};
use flatbuffers::{FlatBufferBuilder, WIPOffset};

pub fn build_tx<'b>(
    fbb: &mut FlatBufferBuilder<'b>,
    tx: &Transaction,
) -> WIPOffset<FbsTransaction<'b>> {
    let vec = tx
        .deps()
        .iter()
        .map(|out_point| FbsOutPoint::build(fbb, out_point))
        .collect::<Vec<_>>();
    let deps = fbb.create_vector(&vec);

    let vec = tx
        .inputs()
        .iter()
        .map(|cell_input| build_input(fbb, cell_input))
        .collect::<Vec<_>>();
    let inputs = fbb.create_vector(&vec);

    let vec = tx
        .outputs()
        .iter()
        .map(|cell_output| build_output(fbb, cell_output))
        .collect::<Vec<_>>();
    let outputs = fbb.create_vector(&vec);

    let mut builder = TransactionBuilder::new(fbb);
    builder.add_version(tx.version());
    builder.add_deps(deps);
    builder.add_inputs(inputs);
    builder.add_outputs(outputs);
    builder.finish()
}

fn build_output<'b>(
    fbb: &mut FlatBufferBuilder<'b>,
    output: &CellOutput,
) -> WIPOffset<FbsCellOutput<'b>> {
    let mut builder = CellOutputBuilder::new(fbb);
    builder.add_capacity(output.capacity.as_u64());
    builder.finish()
}

fn build_input<'b>(
    fbb: &mut FlatBufferBuilder<'b>,
    input: &CellInput,
) -> WIPOffset<FbsCellInput<'b>> {
    let tx_hash = input
        .previous_output
        .cell
        .clone()
        .map(|cell| (&cell.tx_hash).into());
    let tx_index = input.previous_output.cell.as_ref().map(|cell| cell.index);
    let block_hash = input
        .previous_output
        .block_hash
        .clone()
        .map(|hash| (&hash).into());

    let mut builder = CellInputBuilder::new(fbb);
    if let Some(ref hash) = tx_hash {
        builder.add_tx_hash(hash);
    }
    if let Some(index) = tx_index {
        builder.add_index(index);
    }
    if let Some(ref hash) = block_hash {
        builder.add_block_hash(hash);
    }
    builder.add_since(input.since);
    builder.finish()
}
