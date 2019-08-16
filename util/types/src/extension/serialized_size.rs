use crate::{packed, prelude::*};

macro_rules! impl_serialized_size_for_reader {
    ($reader:ident) => {
        impl<'r> packed::$reader<'r> {
            pub fn serialized_size(&self) -> usize {
                self.as_slice().len()
            }
        }
    };
}

macro_rules! impl_serialized_size_for_entity {
    ($entity:ident) => {
        impl packed::$entity {
            pub fn serialized_size(&self) -> usize {
                self.as_reader().serialized_size()
            }
        }
    };
}

macro_rules! impl_serialized_size_for_both {
    ($entity:ident, $reader:ident) => {
        impl_serialized_size_for_reader!($reader);
        impl_serialized_size_for_entity!($entity);
    };
}

impl_serialized_size_for_both!(Block, BlockReader);

impl<'r> packed::TransactionReader<'r> {
    pub fn serialized_size(&self) -> usize {
        // the offset in TransactionVec header is u32
        self.as_slice().len() + 4
    }
}
impl_serialized_size_for_entity!(Transaction);
