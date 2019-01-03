use numext_fixed_hash::H256;
use std::fmt;

pub mod node;
pub mod tree;

// AVL Errors.
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum AVLError {
    // database error,
    DatabaseError(H256),
}

impl fmt::Display for AVLError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            AVLError::DatabaseError(ref key) => write!(f, "Can not find key: {} in DB", key),
        }
    }
}

pub type Result<T> = ::std::result::Result<T, Box<AVLError>>;
