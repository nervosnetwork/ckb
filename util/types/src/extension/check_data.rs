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
