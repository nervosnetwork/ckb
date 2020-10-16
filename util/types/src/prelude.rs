//! This module includes several traits.
//!
//! Few traits are re-exported from other crates, few are used as aliases and others are syntactic sugar.

pub use molecule::{
    hex_string,
    prelude::{Builder, Entity, Reader},
};

/// An alias of `unwrap()` to mark where we are really have confidence to do unwrap.
///
/// We can also customize the panic message or do something else in this alias.
pub trait ShouldBeOk<T> {
    /// Unwraps an `Option` or a `Result` with confidence and we assume that it's impossible to fail.
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
        self.unwrap_or_else(|err| panic!("verify slice should be ok, but {}", err))
    }
}

/// An alias of `from_slice(..)` to mark where we are really have confidence to do unwrap on the result of `from_slice(..)`.
pub trait FromSliceShouldBeOk<'r>: Reader<'r> {
    /// Unwraps the result of `from_slice(..)` with confidence and we assume that it's impossible to fail.
    fn from_slice_should_be_ok(slice: &'r [u8]) -> Self;
}

impl<'r, R> FromSliceShouldBeOk<'r> for R
where
    R: Reader<'r>,
{
    fn from_slice_should_be_ok(slice: &'r [u8]) -> Self {
        match Self::from_slice(slice) {
            Ok(ret) => ret,
            Err(err) => panic!(
                "failed to convert from slice: reason: {}; data: 0x{}.",
                err,
                hex_string(slice)
            ),
        }
    }
}

/// A syntactic sugar to convert binary data into rust types.
pub trait Unpack<T> {
    /// Unpack binary data into rust types.
    fn unpack(&self) -> T;
}

/// A syntactic sugar to convert a rust type into binary data.
pub trait Pack<T: Entity> {
    /// Packs a rust type into binary data.
    fn pack(&self) -> T;
}

/// A syntactic sugar to convert a vector of binary data into one binary data.
pub trait PackVec<T: Entity, I: Entity>: IntoIterator<Item = I> {
    /// Packs a vector of binary data into one binary data.
    fn pack(self) -> T;
}
