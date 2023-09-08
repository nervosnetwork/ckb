use ckb_error::OtherError;

use crate::packed;

/// The DepType enum represents different types of dependencies for `cell_deps`.
#[derive(Default, Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum DepType {
    /// Code dependency: The cell provides code directly
    #[default]
    Code = 0,
    /// Dependency group: The cell bundles several cells as its members
    /// which will be expanded inside `cell_deps`.
    DepGroup = 1,
}

impl TryFrom<packed::Byte> for DepType {
    type Error = OtherError;

    fn try_from(v: packed::Byte) -> Result<Self, Self::Error> {
        match Into::<u8>::into(v) {
            0 => Ok(DepType::Code),
            1 => Ok(DepType::DepGroup),
            _ => Err(OtherError::new(format!("Invalid dep type {v}"))),
        }
    }
}

impl Into<u8> for DepType {
    #[inline]
    fn into(self) -> u8 {
        self as u8
    }
}
impl Into<packed::Byte> for DepType {
    #[inline]
    fn into(self) -> packed::Byte {
        (self as u8).into()
    }
}
