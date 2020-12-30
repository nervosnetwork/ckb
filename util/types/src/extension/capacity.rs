use ckb_occupied_capacity::Result as CapacityResult;

use crate::{core::Capacity, packed, prelude::*};

impl packed::Script {
    /// Calculates the occupied capacity of [`Script`].
    ///
    /// Includes [`code_hash`] (32), [`hash_type`] (1) and [`args`] (calculated).
    ///
    /// [`Script`]: https://github.com/nervosnetwork/ckb/blob/v0.36.0/util/types/schemas/blockchain.mol#L30-L34
    /// [`code_hash`]: #method.code_hash
    /// [`hash_type`]: #method.hash_type
    /// [`args`]: #method.args
    pub fn occupied_capacity(&self) -> CapacityResult<Capacity> {
        Capacity::bytes(self.args().raw_data().len() + 32 + 1)
    }
}

impl packed::CellOutput {
    /// Calculates the occupied capacity of [`CellOutput`].
    ///
    /// Includes [`output_data`] (provided), [`capacity`] (8), [`lock`] (calculated) and [`type`] (calculated).
    ///
    /// [`CellOutput`]: https://github.com/nervosnetwork/ckb/blob/v0.36.0/util/types/schemas/blockchain.mol#L46-L50
    /// [`output_data`]: https://github.com/nervosnetwork/ckb/blob/v0.36.0/util/types/schemas/blockchain.mol#L63
    /// [`capacity`]: #method.capacity
    /// [`lock`]: #method.lock
    /// [`type`]: #method.type_
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

    /// Returns if the [`capacity`] in `CellOutput` is smaller than the [`occupied capacity`].
    ///
    /// [`capacity`]: #method.capacity
    /// [`occupied capacity`]: #method.occupied_capacity
    pub fn is_lack_of_capacity(&self, data_capacity: Capacity) -> CapacityResult<bool> {
        self.occupied_capacity(data_capacity)
            .map(|cap| cap > self.capacity().unpack())
    }
}

impl packed::CellOutputBuilder {
    /// Build a [`CellOutput`] and sets its [`capacity`] equal to its [`occupied capacity`] exactly.
    ///
    /// [`CellOutput`]: struct.CellOutput.html
    /// [`capacity`]: #method.capacity
    /// [`occupied capacity`]: struct.CellOutput.html#method.occupied_capacity
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
    /// Sums the capacities of all [`CellOutput`]s in the vector.
    ///
    /// [`CellOutput`]: struct.CellOutput.html#method.occupied_capacity
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
            (vec![0], 1 + 32 + 1),
            (vec![1], 1 + 32 + 1),
            (vec![0, 0], 2 + 32 + 1),
        ];
        for (args, ckb) in testcases.into_iter() {
            let script = packed::Script::new_builder().args(args.pack()).build();
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
            .args(vec![0u8; 20].pack())
            .build();
        let output = packed::CellOutput::new_builder().lock(lock).build();
        assert_eq!(
            output.occupied_capacity(Capacity::zero()).unwrap(),
            capacity_bytes!(61)
        );
    }
}
