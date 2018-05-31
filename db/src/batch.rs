pub type Col = Option<u32>;

#[derive(Debug, Clone)]
pub enum Operation {
    Insert {
        col: Col,
        key: Vec<u8>,
        value: Vec<u8>,
    },
    Delete {
        col: Col,
        key: Vec<u8>,
    },
}

impl Operation {
    pub fn insert(col: Col, key: Vec<u8>, value: Vec<u8>) -> Self {
        Operation::Insert { col, key, value }
    }

    pub fn delete(col: Col, key: Vec<u8>) -> Self {
        Operation::Delete { col, key }
    }
}

#[derive(Debug, Default, Clone)]
pub struct Batch {
    pub operations: Vec<Operation>,
}

impl Batch {
    pub fn new() -> Self {
        Batch {
            operations: Vec::new(),
        }
    }

    pub fn insert(&mut self, col: Col, key: Vec<u8>, value: Vec<u8>) {
        self.operations.push(Operation::insert(col, key, value));
    }

    pub fn delete(&mut self, col: Col, key: Vec<u8>) {
        self.operations.push(Operation::delete(col, key));
    }
}
