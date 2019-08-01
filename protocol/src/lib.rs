mod builder;
mod convert;
pub mod error;
#[rustfmt::skip]
#[allow(clippy::all)]
#[allow(unused_imports)]
mod protocol_generated;
#[rustfmt::skip]
mod protocol_generated_verifier;

pub use crate::protocol_generated::ckb::protocol::*;
pub use flatbuffers;

pub const DEP_TYPE_CELL: u8 = 0;
pub const DEP_TYPE_CELL_WITH_HEADER: u8 = 1;
pub const DEP_TYPE_DEP_GROUP: u8 = 2;
pub const DEP_TYPE_HEADER: u8 = 3;

pub fn get_root<'a, T>(data: &'a [u8]) -> Result<T::Inner, error::Error>
where
    T: flatbuffers::Follow<'a> + 'a,
    T::Inner: flatbuffers_verifier::Verify,
{
    flatbuffers_verifier::get_root::<T>(data).map_err(|_| error::Error::Malformed)
}

pub struct FlatbuffersVectorIterator<'a, T: flatbuffers::Follow<'a> + 'a> {
    vector: flatbuffers::Vector<'a, T>,
    counter: usize,
}

impl<'a, T: flatbuffers::Follow<'a> + 'a> FlatbuffersVectorIterator<'a, T> {
    pub fn new(vector: flatbuffers::Vector<'a, T>) -> Self {
        Self { vector, counter: 0 }
    }
}

impl<'a, T: flatbuffers::Follow<'a> + 'a> Iterator for FlatbuffersVectorIterator<'a, T> {
    type Item = T::Inner;

    fn next(&mut self) -> Option<Self::Item> {
        if self.counter < self.vector.len() {
            let result = self.vector.get(self.counter);
            self.counter += 1;
            Some(result)
        } else {
            None
        }
    }
}

#[macro_export]
macro_rules! cast {
    ($expr:expr) => {
        $expr.ok_or_else(|| $crate::error::Error::Malformed)
    };
}
