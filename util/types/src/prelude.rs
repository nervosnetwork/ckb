pub use molecule::{
    hex_string,
    prelude::{Builder, Entity, Reader},
};

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
        self.unwrap_or_else(|err| panic!("verify slice should be ok, but {}", err))
    }
}

pub trait FromSliceShouldBeOk<'r>: Reader<'r> {
    fn from_slice_should_be_ok(slice: &'r [u8]) -> Self;
}

impl<'r, R> FromSliceShouldBeOk<'r> for R
where
    R: Reader<'r>,
{
    #[cfg(debug_assertions)]
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

    #[cfg(not(debug_assertions))]
    fn from_slice_should_be_ok(slice: &'r [u8]) -> Self {
        Self::new_unchecked(slice)
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
