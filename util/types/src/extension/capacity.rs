use ckb_occupied_capacity::Result as CapacityResult;

use crate::{core::Capacity, packed, prelude::*};

impl packed::Script {
    pub fn occupied_capacity(&self) -> CapacityResult<Capacity> {
        Capacity::bytes(
            self.args()
                .into_iter()
                .map(|arg| arg.as_reader().raw_data().len())
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
    pub fn build_exact_capacity(
        self,
        data_capacity: Capacity,
    ) -> CapacityResult<packed::CellOutput> {
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
            .map(|x| self.capacity(x.pack()).build())
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

#[cfg(test)]
mod tests {
    use crate::{
        core::{capacity_bytes, Capacity},
        packed,
        prelude::*,
    };

    #[test]
    fn script_occupied_capacity() {
        let testcases = vec![
            (vec![], 32 + 1),
            (vec![vec![]], 32 + 1),
            (vec![vec![0u8]], 1 + 32 + 1),
            (vec![vec![1]], 1 + 32 + 1),
            (vec![vec![0, 0]], 2 + 32 + 1),
            (vec![vec![0], vec![0]], 2 + 32 + 1),
        ];
        for (args, ckb) in testcases.into_iter() {
            let script = packed::Script::new_builder()
                .args(
                    args.into_iter()
                        .map(|x| x.pack())
                        .collect::<Vec<packed::Bytes>>()
                        .pack(),
                )
                .build();
            let expect = Capacity::bytes(ckb).unwrap();
            assert_eq!(script.occupied_capacity().unwrap(), expect);
        }
    }

    #[test]
    fn min_cell_output_capacity() {
        let lock = packed::Script::new_builder().build();
        let output = packed::CellOutput::new_builder().lock(lock).build();
        assert_eq!(
            output.occupied_capacity(Capacity::zero()).unwrap(),
            capacity_bytes!(41)
        );
    }

    #[test]
    fn min_secp256k1_cell_output_capacity() {
        let lock = packed::Script::new_builder()
            .args(vec![vec![0u8; 20].pack()].pack())
            .build();
        let output = packed::CellOutput::new_builder().lock(lock).build();
        assert_eq!(
            output.occupied_capacity(Capacity::zero()).unwrap(),
            capacity_bytes!(61)
        );
    }
}
