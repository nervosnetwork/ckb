use crate::{core, packed};

/*
 * Blockchain
 */

impl<'r> packed::ScriptReader<'r> {
    fn check_data(&self) -> bool {
        core::ScriptHashType::verify_value(self.hash_type().into())
    }
}

impl<'r> packed::ScriptOptReader<'r> {
    fn check_data(&self) -> bool {
        self.to_opt()
            .map(|i| core::ScriptHashType::verify_value(i.hash_type().into()))
            .unwrap_or(true)
    }
}

impl<'r> packed::CellOutputReader<'r> {
    fn check_data(&self) -> bool {
        self.lock().check_data() && self.type_().check_data()
    }
}

impl<'r> packed::CellOutputVecReader<'r> {
    fn check_data(&self) -> bool {
        self.iter().all(|i| i.check_data())
    }
}

impl<'r> packed::CellDepReader<'r> {
    fn check_data(&self) -> bool {
        core::DepType::verify_value(self.dep_type().into())
    }
}

impl<'r> packed::CellDepVecReader<'r> {
    fn check_data(&self) -> bool {
        self.iter().all(|i| i.check_data())
    }
}

impl<'r> packed::RawTransactionReader<'r> {
    fn check_data(&self) -> bool {
        self.outputs().len() == self.outputs_data().len()
            && self.cell_deps().check_data()
            && self.outputs().check_data()
    }
}

impl<'r> packed::TransactionReader<'r> {
    fn check_data(&self) -> bool {
        self.raw().check_data()
    }
}

impl<'r> packed::TransactionVecReader<'r> {
    fn check_data(&self) -> bool {
        self.iter().all(|i| i.check_data())
    }
}

impl<'r> packed::BlockReader<'r> {
    fn check_data(&self) -> bool {
        self.transactions().check_data()
    }
}

/*
 * Network
 */

impl<'r> packed::BlockTransactionsReader<'r> {
    /// Recursively checks whether the structure of the binary data is correct.
    pub fn check_data(&self) -> bool {
        self.transactions().check_data()
    }
}

impl<'r> packed::RelayTransactionReader<'r> {
    fn check_data(&self) -> bool {
        self.transaction().check_data()
    }
}

impl<'r> packed::RelayTransactionVecReader<'r> {
    fn check_data(&self) -> bool {
        self.iter().all(|i| i.check_data())
    }
}

impl<'r> packed::RelayTransactionsReader<'r> {
    /// Recursively checks whether the structure of the binary data is correct.
    pub fn check_data(&self) -> bool {
        self.transactions().check_data()
    }
}

impl<'r> packed::SendBlockReader<'r> {
    /// Recursively checks whether the structure of the binary data is correct.
    pub fn check_data(&self) -> bool {
        self.block().check_data()
    }
}

#[cfg(test)]
mod tests {
    use crate::{packed, prelude::*};

    fn create_transaction(
        outputs: &[&packed::CellOutput],
        outputs_data: &[&[u8]],
        cell_deps: &[&packed::CellDep],
    ) -> packed::Transaction {
        let outputs = outputs
            .iter()
            .map(|d| d.to_owned().to_owned())
            .collect::<Vec<packed::CellOutput>>();
        let outputs_data = outputs_data
            .iter()
            .map(|d| d.to_owned().to_owned().pack())
            .collect::<Vec<packed::Bytes>>();
        let cell_deps = cell_deps
            .iter()
            .map(|d| d.to_owned().to_owned())
            .collect::<Vec<packed::CellDep>>();
        let raw = packed::RawTransaction::new_builder()
            .outputs(outputs.into_iter().pack())
            .outputs_data(outputs_data.into_iter().pack())
            .cell_deps(cell_deps.into_iter().pack())
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
        let ht_right = 1.into();
        let ht_error = 2.into();
        let dt_right = 1.into();
        let dt_error = 2.into();

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
        test_check_data_via_transaction(true, &[&output_right1], &[&[]], &[&cell_dep_right]);
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
