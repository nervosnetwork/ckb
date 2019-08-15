pub use molecule::prelude::{Builder, Entity, Reader};

use crate::H256;

// An alias for unwrap / expect.
pub trait ShouldBeOk<T> {
    fn should_be_ok(self) -> T;
}

// Use for Option
impl<T> ShouldBeOk<T> for Option<T> {
    fn should_be_ok(self) -> T {
        self.unwrap_or_else(|| panic!("should not be None"))
    }
}

// Use for verify
impl<T> ShouldBeOk<T> for molecule::error::VerificationResult<T> {
    fn should_be_ok(self) -> T {
        self.unwrap_or_else(|err| panic!("verify slice should be ok, but {:?}", err))
    }
}

pub trait Unpack<T> {
    fn unpack(&self) -> T;
}

pub trait Pack<T: Entity> {
    fn pack(&self) -> T;
}

pub trait PackVec<T: Entity, I: Entity>: IntoIterator<Item = I> {
    fn pack(self) -> T;
}

pub trait CalcHash {
    fn calc_hash(&self) -> H256;
}

pub trait SerializedSize {
    fn serialized_size(&self) -> usize;
}
