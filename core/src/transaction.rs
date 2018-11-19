//! Transaction using Cell.
//! It is similar to Bitcoin Tx <https://en.bitcoin.it/wiki/Protocol_documentation#tx/>
use bigint::H256;
use bincode::serialize;
use hash::sha3_256;
use std::slice::Iter;

#[derive(Clone, Default, Serialize, Deserialize, Eq, PartialEq, Hash, Debug)]
pub struct OutPoint {
    // Hash of Transaction
    pub hash: H256,
    // Index of cell_operations
    pub index: u32,
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Debug)]
pub struct Recipient {
    pub module: u32,
    pub lock: Vec<u8>,
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Debug)]
pub struct CellInput {
    pub previous_output: OutPoint,
    // Depends on whether the operation is Transform or Destroy, this is the proof to transform
    // lock or destroy lock.
    pub unlock: Vec<u8>,
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Debug)]
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
#[derive(Clone, Serialize, Deserialize, PartialEq, Debug, Default)]
pub struct OperationGrouping {
    pub transform_count: u32,
    pub destroy_count: u32,
    pub create_count: u32,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Debug, Default)]
pub struct Transaction {
    pub version: u32,
    pub inputs: Vec<CellInput>,
    pub outputs: Vec<CellOutput>,

    // Number of operations in each group. Sum of the numbers must equal to the size of operations
    // list.
    pub groupings: Vec<OperationGrouping>,
}

#[derive(PartialEq, Debug)]
pub struct OperationGroup<'a> {
    pub transform_inputs: &'a [CellInput],
    pub transform_outputs: &'a [CellOutput],
    pub create_outputs: &'a [CellOutput],
    pub destroy_inputs: &'a [CellInput],
}

#[derive(Debug)]
pub struct OperationGroupIter<'a> {
    inputs_slice: &'a [CellInput],
    outputs_slice: &'a [CellOutput],
    groupings_iter: Iter<'a, OperationGrouping>,
}

impl CellOutput {
    pub fn bytes_len(&self) -> usize {
        8 + self.data.len() + self.lock.len() + self.recipient.as_ref().map_or(0, |r| r.bytes_len())
    }
}

impl Recipient {
    pub fn bytes_len(&self) -> usize {
        4 + self.lock.len()
    }
}

impl Transaction {
    // TODO: split it
    // TODO: tells validation error
    pub fn validate(&self, is_enlarge_transaction: bool) -> bool {
        if is_enlarge_transaction && !(self.inputs.is_empty() && self.outputs.len() == 1) {
            return false;
        }

        // check outputs capacity
        for output in &self.outputs {
            if output.bytes_len() > (output.capacity as usize) {
                return false;
            }
        }

        // check grouping
        let mut transform_count = 0;
        let mut destroy_count = 0;
        let mut create_count = 0;
        for grouping in &self.groupings {
            if grouping.transform_count == 0 && grouping.destroy_count == 0
                && grouping.create_count == 0
            {
                return false;
            }
            transform_count += grouping.transform_count;
            destroy_count += grouping.destroy_count;
            create_count += grouping.create_count;
        }

        if (transform_count + destroy_count) as usize != self.inputs.len()
            || (transform_count + create_count) as usize != self.outputs.len()
        {
            return false;
        }

        true
    }

    pub fn hash(&self) -> H256 {
        sha3_256(serialize(self).unwrap()).into()
    }

    pub fn groups_iter(&self) -> OperationGroupIter {
        OperationGroupIter {
            inputs_slice: &self.inputs[..],
            outputs_slice: &self.outputs[..],
            groupings_iter: self.groupings.iter(),
        }
    }

    pub fn check_lock(&self, unlock: &[u8], lock: &[u8]) -> bool {
        // TODO: check using pubkey signature
        unlock.is_empty() || !lock.is_empty()
    }
}

impl<'a> Iterator for OperationGroupIter<'a> {
    type Item = OperationGroup<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.groupings_iter.next() {
            Some(grouping) => {
                let transform_count = grouping.transform_count as usize;
                let consumed_inputs_count =
                    (grouping.transform_count + grouping.destroy_count) as usize;
                let consumed_outputs_count =
                    (grouping.transform_count + grouping.create_count) as usize;

                let group = OperationGroup {
                    transform_inputs: &self.inputs_slice[0..transform_count],
                    transform_outputs: &self.outputs_slice[0..transform_count],
                    destroy_inputs: &self.inputs_slice[transform_count..consumed_inputs_count],
                    create_outputs: &self.outputs_slice[transform_count..consumed_outputs_count],
                };

                self.inputs_slice = &self.inputs_slice[consumed_inputs_count..];
                self.outputs_slice = &self.outputs_slice[consumed_outputs_count..];

                Some(group)
            }
            None => None,
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.groupings_iter.size_hint()
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
    fn empty_groups_iter() {
        let tx = Transaction {
            version: 0,
            inputs: Vec::new(),
            outputs: Vec::new(),
            groupings: Vec::new(),
        };

        let mut iter = tx.groups_iter();
        assert_eq!(iter.next(), None);
    }

    #[test]
    fn groups_iter_happy_pass() {
        let tx = Transaction {
            version: 0,
            inputs: [1u8, 2u8, 3u8].into_iter().map(build_cell_input).collect(),
            outputs: [1u8, 2u8, 3u8, 4u8]
                .into_iter()
                .map(build_cell_output)
                .collect(),
            groupings: vec![
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

        let mut iter = tx.groups_iter();
        if let Some(group) = iter.next() {
            assert_eq!(1, group.transform_inputs.len());
            assert_eq!(1, group.transform_outputs.len());
            // i1 -> o1
            assert_eq!(1, group.transform_inputs[0].unlock[0]);
            assert_eq!(1, group.transform_outputs[0].lock[0]);

            assert_eq!(1, group.destroy_inputs.len());
            // i2 -> x
            assert_eq!(2, group.destroy_inputs[0].unlock[0]);

            assert_eq!(2, group.create_outputs.len());
            // x -> o2
            assert_eq!(2, group.create_outputs[0].lock[0]);
            // x -> o3
            assert_eq!(3, group.create_outputs[1].lock[0]);
        } else {
            panic!("Expect 2 groups, got 0");
        }

        if let Some(group) = iter.next() {
            assert_eq!(1, group.transform_inputs.len());
            assert_eq!(1, group.transform_outputs.len());
            // i3 -> o4
            assert_eq!(3, group.transform_inputs[0].unlock[0]);
            assert_eq!(4, group.transform_outputs[0].lock[0]);

            assert_eq!(0, group.destroy_inputs.len());
            assert_eq!(0, group.create_outputs.len());
        } else {
            panic!("Expect 2 groups, got 1");
        }

        assert_eq!(iter.next(), None, "Expect 2 groups, got more");
    }
}
