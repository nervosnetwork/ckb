use crate::{
    borrow::ToOwned,
    packed::{self, Bytes, CellOutput},
    prelude::*,
};

fn create_transaction(
    outputs: &[&packed::CellOutput],
    outputs_data: &[&[u8]],
    cell_deps: &[&packed::CellDep],
) -> packed::Transaction {
    let outputs_iter: Vec<CellOutput> = outputs.iter().map(|d| d.to_owned().to_owned()).collect();
    let outputs_data_iter: Vec<Bytes> = outputs_data
        .iter()
        .map(|d| Into::<Bytes>::into(d.to_owned()))
        .collect();
    let cell_deps_iter: Vec<packed::CellDep> =
        cell_deps.iter().map(|d| d.to_owned().to_owned()).collect();
    let raw = packed::RawTransaction::new_builder()
        .outputs(outputs_iter)
        .outputs_data(outputs_data_iter)
        .cell_deps(cell_deps_iter)
        .build();
    packed::Transaction::new_builder().raw(raw).build()
}

fn test_check_data_via_transaction(
    expected: bool,
    outputs: &[&packed::CellOutput],
    outputs_data: &[&[u8]],
    cell_deps: &[&packed::CellDep],
) {
    let tx = create_transaction(outputs, outputs_data, cell_deps);
    assert_eq!(tx.as_reader().check_data(), expected);
}

#[test]
fn check_data() {
    for ht in 0..4 {
        if ht != 3 {
            for dt in 0..2 {
                let ht_right = ht;
                let dt_right = dt;
                let ht_error = 3;
                let dt_error = 2;

                let script_right = packed::Script::new_builder().hash_type(ht_right).build();
                let script_error = packed::Script::new_builder().hash_type(ht_error).build();

                let script_opt_right = packed::ScriptOpt::new_builder()
                    .set(Some(script_right.clone()))
                    .build();
                let script_opt_error = packed::ScriptOpt::new_builder()
                    .set(Some(script_error.clone()))
                    .build();

                let output_right1 = packed::CellOutput::new_builder()
                    .lock(script_right.clone())
                    .build();
                let output_right2 = packed::CellOutput::new_builder()
                    .type_(script_opt_right.clone())
                    .build();
                let output_error1 = packed::CellOutput::new_builder()
                    .lock(script_error.clone())
                    .build();
                let output_error2 = packed::CellOutput::new_builder()
                    .type_(script_opt_error.clone())
                    .build();
                let output_error3 = packed::CellOutput::new_builder()
                    .lock(script_right)
                    .type_(script_opt_error)
                    .build();
                let output_error4 = packed::CellOutput::new_builder()
                    .lock(script_error)
                    .type_(script_opt_right)
                    .build();

                let cell_dep_right = packed::CellDep::new_builder().dep_type(dt_right).build();
                let cell_dep_error = packed::CellDep::new_builder().dep_type(dt_error).build();

                test_check_data_via_transaction(true, &[], &[], &[]);
                test_check_data_via_transaction(
                    true,
                    &[&output_right1],
                    &[&[]],
                    &[&cell_dep_right],
                );
                test_check_data_via_transaction(
                    true,
                    &[&output_right1, &output_right2],
                    &[&[], &[]],
                    &[&cell_dep_right, &cell_dep_right],
                );
                test_check_data_via_transaction(false, &[&output_error1], &[&[]], &[]);
                test_check_data_via_transaction(false, &[&output_error2], &[&[]], &[]);
                test_check_data_via_transaction(false, &[&output_error3], &[&[]], &[]);
                test_check_data_via_transaction(false, &[&output_error4], &[&[]], &[]);
                test_check_data_via_transaction(false, &[], &[], &[&cell_dep_error]);
                test_check_data_via_transaction(
                    false,
                    &[
                        &output_right1,
                        &output_right2,
                        &output_error1,
                        &output_error2,
                        &output_error3,
                        &output_error4,
                    ],
                    &[&[], &[], &[], &[], &[], &[]],
                    &[&cell_dep_right, &cell_dep_error],
                );
                test_check_data_via_transaction(false, &[&output_right1], &[], &[&cell_dep_right]);
                test_check_data_via_transaction(false, &[], &[&[]], &[&cell_dep_right]);
            }
        }
    }
}
