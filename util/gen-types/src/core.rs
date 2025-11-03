//! The essential rust types for CKB contracts.

#![allow(clippy::from_over_into)]

use crate::packed;
use seq_macro::seq;
use strum::FromRepr;

seq!(N in 3..=127 {
    /// Specifies how the script `code_hash` is used to match the script code and how to run the code.
    /// The hash type is split into the high 7 bits and the low 1 bit,
    /// when the low 1 bit is 1, it indicates the type,
    /// when the low 1 bit is 0, it indicates the data,
    /// and then it relies on the high 7 bits to indicate
    /// that the data actually corresponds to the version.
     #[derive(Default, Clone, Copy, PartialEq, Eq, Debug, Hash, FromRepr)]
     #[repr(u8)]
    pub enum ScriptHashType {
        /// Type "type" matches script code via cell type script hash.
        Type = 1,
        /// Type "data" matches script code via cell data hash, and run the script code in v0 CKB VM.
        #[default]
        Data = 0,
        /// Type "data1" matches script code via cell data hash, and run the script code in v1 CKB VM.
        Data1 = 2,
        /// Type "data2" matches script code via cell data hash, and run the script code in v2 CKB VM.
        Data2 = 4,
        #(
            #[doc = concat!("Type \"data", stringify!(N), "\" matches script code via cell data hash, and runs the script code in v", stringify!(N), " CKB VM.")]
            Data~N = N << 1,
        )*
    }
});

impl ScriptHashType {
    /// when the low 1 bit is 1, it indicates the type
    /// when the low 1 bit is 0, it indicates the data
    #[inline]
    pub fn verify_value(v: u8) -> bool {
        v.is_multiple_of(2) || v == 1
    }
}

impl Into<u8> for ScriptHashType {
    #[inline]
    fn into(self) -> u8 {
        self as u8
    }
}

impl Into<packed::Byte> for ScriptHashType {
    #[inline]
    fn into(self) -> packed::Byte {
        Into::<u8>::into(self).into()
    }
}

#[cfg(feature = "std")]
mod std_mod {
    use crate::{core::ScriptHashType, packed};
    use ckb_error::OtherError;

    impl TryFrom<u8> for ScriptHashType {
        type Error = OtherError;
        fn try_from(v: u8) -> Result<Self, Self::Error> {
            ScriptHashType::from_repr(v)
                .ok_or(OtherError::new(format!("Invalid script hash type {v}")))
        }
    }

    impl TryFrom<packed::Byte> for ScriptHashType {
        type Error = OtherError;

        fn try_from(v: packed::Byte) -> Result<Self, Self::Error> {
            Into::<u8>::into(v).try_into()
        }
    }
}

#[cfg(test)]
mod test {
    use crate::core::ScriptHashType;
    #[test]
    fn test_into_u8() {
        assert_eq!(Into::<u8>::into(ScriptHashType::Data), 0u8);
        assert_eq!(Into::<u8>::into(ScriptHashType::Data1), 2u8);
        assert_eq!(Into::<u8>::into(ScriptHashType::Data2), 4u8);
        assert_eq!(Into::<u8>::into(ScriptHashType::Data3), 6u8);
        assert_eq!(Into::<u8>::into(ScriptHashType::Data127), 254u8);
    }

    #[test]
    fn test_from_u8() {
        assert!(ScriptHashType::try_from(3).is_err());
        assert_eq!(ScriptHashType::try_from(2), Ok(ScriptHashType::Data1));
        assert_eq!(ScriptHashType::try_from(4), Ok(ScriptHashType::Data2));
        assert_eq!(ScriptHashType::try_from(6), Ok(ScriptHashType::Data3));
        assert_eq!(ScriptHashType::try_from(254), Ok(ScriptHashType::Data127));
    }

    #[test]
    fn test_verify_value() {
        assert!(ScriptHashType::verify_value(0b0000_0000));
        assert!(ScriptHashType::verify_value(0b0000_0001));
        assert!(ScriptHashType::verify_value(0b1010_1010));

        // invalid
        assert!(!ScriptHashType::verify_value(0b0000_0011));
        assert!(!ScriptHashType::verify_value(0b1000_0001));
        assert!(!ScriptHashType::verify_value(0b1111_1111));
        assert!(!ScriptHashType::verify_value(0b0000_0010 | 0b0000_0001));
    }
}
