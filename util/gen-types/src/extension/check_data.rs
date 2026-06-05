use crate::{core, packed};
use molecule::prelude::Reader;

/*
 * Blockchain
 */

const MAX_DEP_TYPE: u8 = 1;

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
        MAX_DEP_TYPE >= self.dep_type().into()
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
    pub(crate) fn check_data(&self) -> bool {
        self.raw().check_data()
    }
}

impl<'r> packed::TransactionVecReader<'r> {
    fn check_data(&self) -> bool {
        self.iter().all(|i| i.check_data())
    }
}

fn extra_fields_are_valid_bytes(slice: &[u8], field_count: usize, extra_count: usize) -> bool {
    (0..extra_count).all(|index| {
        let offset_index = (1 + field_count + index) * molecule::NUMBER_SIZE;
        let start = molecule::unpack_number(&slice[offset_index..]) as usize;
        let end = if index + 1 == extra_count {
            molecule::unpack_number(slice) as usize
        } else {
            let next_offset_index = offset_index + molecule::NUMBER_SIZE;
            molecule::unpack_number(&slice[next_offset_index..]) as usize
        };
        packed::BytesReader::verify(&slice[start..end], false).is_ok()
    })
}

impl<'r> packed::BlockReader<'r> {
    fn check_data(&self) -> bool {
        self.transactions().check_data()
            && extra_fields_are_valid_bytes(
                self.as_slice(),
                Self::FIELD_COUNT,
                self.count_extra_fields(),
            )
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

impl<'r> packed::IndexTransactionReader<'r> {
    fn check_data(&self) -> bool {
        self.transaction().check_data()
    }
}

impl<'r> packed::IndexTransactionVecReader<'r> {
    fn check_data(&self) -> bool {
        self.iter().all(|i| i.check_data())
    }
}

impl<'r> packed::SendBlockReader<'r> {
    /// Recursively checks whether the structure of the binary data is correct.
    pub fn check_data(&self) -> bool {
        self.block().check_data()
    }
}

impl<'r> packed::CompactBlockReader<'r> {
    /// Recursively checks whether the structure of the binary data is correct.
    pub fn check_data(&self) -> bool {
        self.prefilled_transactions().check_data()
            && extra_fields_are_valid_bytes(
                self.as_slice(),
                Self::FIELD_COUNT,
                self.count_extra_fields(),
            )
    }
}
