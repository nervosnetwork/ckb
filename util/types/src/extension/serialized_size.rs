use crate::{packed, prelude::*};

macro_rules! impl_serialized_size_for_reader {
    ($reader:ident) => {
        impl<'r> SerializedSize for packed::$reader<'r> {
            fn serialized_size(&self) -> usize {
                self.as_slice().len()
            }
        }
    };
}

macro_rules! impl_serialized_size_for_entity {
    ($entity:ident) => {
        impl SerializedSize for packed::$entity {
            fn serialized_size(&self) -> usize {
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

impl<'r> SerializedSize for packed::TransactionReader<'r> {
    fn serialized_size(&self) -> usize {
        self.as_slice().len() + 4 // the offset in TransactionVec header is u32
    }
}
impl_serialized_size_for_entity!(Transaction);
