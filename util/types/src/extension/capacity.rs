use ckb_occupied_capacity::Result as CapacityResult;

use crate::{core::Capacity, packed, prelude::*};

impl packed::Script {
    pub fn occupied_capacity(&self) -> CapacityResult<Capacity> {
        Capacity::bytes(
            self.args()
                .into_iter()
                .map(|arg| arg.as_reader().as_unpack_slice().len())
                .sum::<usize>()
                + 32
                + 1,
        )
    }
}

impl packed::CellOutput {
    pub fn occupied_capacity(&self, data_capacity: Capacity) -> CapacityResult<Capacity> {
        Capacity::bytes(8)
            .and_then(|x| x.safe_add(data_capacity))
            .and_then(|x| self.lock().occupied_capacity().and_then(|y| y.safe_add(x)))
            .and_then(|x| {
                self.type_()
                    .to_opt()
                    .as_ref()
                    .map(packed::Script::occupied_capacity)
                    .transpose()
                    .and_then(|y| y.unwrap_or_else(Capacity::zero).safe_add(x))
            })
    }

    pub fn is_lack_of_capacity(&self, data_capacity: Capacity) -> CapacityResult<bool> {
        self.occupied_capacity(data_capacity)
            .map(|cap| cap > self.capacity().unpack())
    }
}

impl packed::CellOutputBuilder {
    pub fn reset_capacity(self, data_capacity: Capacity) -> CapacityResult<Self> {
        Capacity::bytes(8)
            .and_then(|x| x.safe_add(data_capacity))
            .and_then(|x| self.lock.occupied_capacity().and_then(|y| y.safe_add(x)))
            .and_then(|x| {
                self.type_
                    .to_opt()
                    .as_ref()
                    .map(packed::Script::occupied_capacity)
                    .transpose()
                    .and_then(|y| y.unwrap_or_else(Capacity::zero).safe_add(x))
            })
            .map(|x| self.capacity(x.pack()))
    }
}

impl packed::CellOutputVec {
    pub fn total_capacity(&self) -> CapacityResult<Capacity> {
        self.as_reader()
            .iter()
            .map(|output| {
                let cap: Capacity = output.capacity().unpack();
                cap
            })
            .try_fold(Capacity::zero(), Capacity::safe_add)
    }
}
