//! Transaction using Cell.
//! It is similar to Bitcoin Tx <https://en.bitcoin.it/wiki/Protocol_documentation#tx/>
use bigint::H256;
use bincode::serialize;
use hash::sha3_256;
use std::iter::Zip;
use std::slice::Iter;

#[derive(Clone, Serialize, Deserialize, PartialEq, Debug)]
pub struct OutPoint {
    // Hash of Transaction
    pub hash: H256,
    // Index of cell_operations
    pub index: u32,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Debug)]
pub struct Recipient {
    pub module_id: u32,
    pub lock: Vec<u8>,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Debug)]
pub struct CellInput {
    pub previous_output: OutPoint,
    // Depends on whether the operation is Transform or Destroy, this is the proof to transform
    // lock or destroy lock.
    pub unlock: Vec<u8>,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Debug)]
pub struct CellOutput {
    pub module: u32,
    pub capacity: u32,
    pub data: Vec<u8>,
    pub lock: Vec<u8>,
    pub recipient: Option<Recipient>,
}

// The cell operations are ordered by group.
//
// In each group, transform inputs are ordered before destroy inputs. And transform outputs are
// ordered before create outputs.
//
// For example, a transaction has inputs i1, i2, i3, outputs o1, o2, o3, o4, 2 groups:
//
// - g1: transform_count = 1, destroy_count = 1, create_count = 2
// - g2: transform_count = 1, destroy_count = 0, create_count = 0
//
// Then g1 has operations:
//
// - Transform i1 -> o1
// - Destroy i2 -> x
// - Create x -> o2
// - Create x -> o3
//
// Group g2 has following operations:
//
// - Transform i3 -> o4
#[derive(Clone, Serialize, Deserialize, PartialEq, Debug)]
pub struct OperationGrouping {
    pub transform_count: u32,
    pub destroy_count: u32,
    pub create_count: u32,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Debug)]
pub struct Transaction {
    pub version: u32,
    pub inputs: Vec<CellInput>,
    pub outputs: Vec<CellOutput>,

    // Number of operations in each group. Sum of the numbers must equal to the size of operations
    // list.
    pub grouping: Vec<OperationGrouping>,
}

impl Transaction {
    pub fn validate(&self) -> bool {
        // TODO implement it
        true
    }

    pub fn hash(&self) -> H256 {
        sha3_256(serialize(self).unwrap()).into()
    }
}

#[derive(PartialEq, Debug)]
pub struct OperationGroup<'a> {
    inputs_slice: &'a [CellInput],
    outputs_slice: &'a [CellOutput],
    grouping: &'a OperationGrouping,
}

#[derive(Debug)]
pub struct OperationGroupIter<'a> {
    inputs_slice: &'a [CellInput],
    outputs_slice: &'a [CellOutput],
    grouping_iter: Iter<'a, OperationGrouping>,
}

impl Transaction {
    pub fn group_iter(&self) -> OperationGroupIter {
        OperationGroupIter {
            inputs_slice: &self.inputs[..],
            outputs_slice: &self.outputs[..],
            grouping_iter: self.grouping.iter(),
        }
    }
}

impl<'a> Iterator for OperationGroupIter<'a> {
    type Item = OperationGroup<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.grouping_iter.next() {
            Some(grouping) => {
                let consumed_inputs_count =
                    (grouping.transform_count + grouping.destroy_count) as usize;
                let consumed_outputs_count =
                    (grouping.transform_count + grouping.create_count) as usize;

                let group = OperationGroup {
                    inputs_slice: &self.inputs_slice[0..consumed_inputs_count],
                    outputs_slice: &self.outputs_slice[0..consumed_outputs_count],
                    grouping,
                };

                self.inputs_slice = &self.inputs_slice[consumed_inputs_count..];
                self.outputs_slice = &self.outputs_slice[consumed_outputs_count..];

                Some(group)
            }
            None => None,
        }
    }
}

impl<'a> OperationGroup<'a> {
    pub fn transform_operations(&self) -> Zip<Iter<CellInput>, Iter<CellOutput>> {
        let count = self.grouping.transform_count as usize;
        (&self.inputs_slice[0..count])
            .iter()
            .zip((&self.outputs_slice[0..count]).iter())
    }

    pub fn destroy_operations(&self) -> Iter<CellInput> {
        let start_from = self.grouping.transform_count as usize;
        (&self.inputs_slice[start_from..]).iter()
    }

    pub fn create_operations(&self) -> Iter<CellOutput> {
        let start_from = self.grouping.transform_count as usize;
        (&self.outputs_slice[start_from..]).iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_cell_input(tag: &u8) -> CellInput {
        CellInput {
            previous_output: OutPoint {
                hash: 0.into(),
                index: 0,
            },
            unlock: vec![*tag],
        }
    }

    fn build_cell_output(tag: &u8) -> CellOutput {
        CellOutput {
            module: 0,
            capacity: 0,
            data: vec![],
            lock: vec![*tag],
            recipient: None,
        }
    }

    #[test]
    fn empty_group_iter() {
        let tx = Transaction {
            version: 0,
            inputs: Vec::new(),
            outputs: Vec::new(),
            grouping: Vec::new(),
        };

        let mut iter = tx.group_iter();
        assert_eq!(iter.next(), None);
    }

    #[test]
    fn group_iter_happy_pass() {
        let tx = Transaction {
            version: 0,
            inputs: [1u8, 2u8, 3u8].into_iter().map(build_cell_input).collect(),
            outputs: [1u8, 2u8, 3u8, 4u8]
                .into_iter()
                .map(build_cell_output)
                .collect(),
            grouping: vec![
                OperationGrouping {
                    transform_count: 1,
                    destroy_count: 1,
                    create_count: 2,
                },
                OperationGrouping {
                    transform_count: 1,
                    destroy_count: 0,
                    create_count: 0,
                },
            ],
        };

        let mut iter = tx.group_iter();
        if let Some(group) = iter.next() {
            let transform_operations: Vec<(&CellInput, &CellOutput)> =
                group.transform_operations().collect();
            let destroy_operations: Vec<&CellInput> = group.destroy_operations().collect();
            let create_operations: Vec<&CellOutput> = group.create_operations().collect();

            assert_eq!(1, transform_operations.len());
            // i1 -> o1
            assert_eq!(1, transform_operations[0].0.unlock[0]);
            assert_eq!(1, transform_operations[0].1.lock[0]);

            assert_eq!(1, destroy_operations.len());
            // i2 -> x
            assert_eq!(2, destroy_operations[0].unlock[0]);

            assert_eq!(2, create_operations.len());
            // x -> o2
            assert_eq!(2, create_operations[0].lock[0]);
            // x -> o3
            assert_eq!(3, create_operations[1].lock[0]);
        } else {
            panic!("Expect 2 groups, got 0");
        }

        if let Some(group) = iter.next() {
            let transform_operations: Vec<(&CellInput, &CellOutput)> =
                group.transform_operations().collect();
            let destroy_operations: Vec<&CellInput> = group.destroy_operations().collect();
            let create_operations: Vec<&CellOutput> = group.create_operations().collect();

            assert_eq!(1, transform_operations.len());
            // i3 -> o4
            assert_eq!(3, transform_operations[0].0.unlock[0]);
            assert_eq!(4, transform_operations[0].1.lock[0]);

            assert_eq!(0, destroy_operations.len());
            assert_eq!(0, create_operations.len());
        } else {
            panic!("Expect 2 groups, got 1");
        }

        assert_eq!(iter.next(), None, "Expect 2 groups, got more");
    }
}
