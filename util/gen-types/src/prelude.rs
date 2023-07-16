pub use molecule::prelude::{Builder, Entity, Reader};

/// A syntactic sugar to convert binary data into rust types.
pub trait Unpack<T> {
    fn unpack(&self) -> T;
}

pub trait Pack<T: Entity> {
    fn pack(&self) -> T;
}

pub trait PackVec<T: Entity, I: Entity>: IntoIterator<Item = I> {
    fn pack(self) -> T;
}
