use crate::{H160, H256, H512, H520, error::FromStrError};

pub(crate) const DICT_HEX_ERROR: u8 = u8::MAX;
pub(crate) static DICT_HEX_LO: [u8; 256] = {
    const ____: u8 = DICT_HEX_ERROR;
    [
        ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____,
        ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____,
        ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____,
        ____, ____, ____, 0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, ____, ____,
        ____, ____, ____, ____, ____, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f, ____, ____, ____, ____,
        ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____,
        ____, ____, ____, ____, ____, ____, ____, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f, ____, ____,
        ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____,
        ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____,
        ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____,
        ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____,
        ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____,
        ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____,
        ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____,
        ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____,
        ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____,
        ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____,
        ____,
    ]
};
pub(crate) static DICT_HEX_HI: [u8; 256] = {
    const ____: u8 = DICT_HEX_ERROR;
    [
        ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____,
        ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____,
        ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____,
        ____, ____, ____, 0x00, 0x10, 0x20, 0x30, 0x40, 0x50, 0x60, 0x70, 0x80, 0x90, ____, ____,
        ____, ____, ____, ____, ____, 0xa0, 0xb0, 0xc0, 0xd0, 0xe0, 0xf0, ____, ____, ____, ____,
        ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____,
        ____, ____, ____, ____, ____, ____, ____, 0xa0, 0xb0, 0xc0, 0xd0, 0xe0, 0xf0, ____, ____,
        ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____,
        ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____,
        ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____,
        ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____,
        ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____,
        ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____,
        ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____,
        ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____,
        ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____,
        ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____, ____,
        ____,
    ]
};

macro_rules! impl_std_str_fromstr {
    ($name:ident, $bytes_size:expr) => {
        impl ::std::str::FromStr for $name {
            type Err = FromStrError;
            fn from_str(input: &str) -> Result<Self, Self::Err> {
                let len = input.as_bytes().len();
                if len != $bytes_size * 2 {
                    return Err(FromStrError::InvalidLength(len));
                }
                let mut ret = Self::default();
                for (idx, chr) in input.bytes().enumerate() {
                    let val = if idx % 2 == 0 {
                        DICT_HEX_HI[usize::from(chr)]
                    } else {
                        DICT_HEX_LO[usize::from(chr)]
                    };
                    if val == DICT_HEX_ERROR {
                        return Err(FromStrError::InvalidCharacter { chr, idx });
                    }
                    ret.0[idx / 2] |= val;
                }
                Ok(ret)
            }
        }
    };
}

macro_rules! impl_from_trimmed_str {
    ($name:ident, $bytes_size:expr, $use_stmt:expr, $bytes_size_stmt:expr) => {
        impl $name {
            /// To convert a trimmed hexadecimal string into `Self`.
            ///
            /// If the beginning of a hexadecimal string are one or more zeros, then these zeros
            /// should be omitted.
            ///
            /// There should be only one zero at the beginning of a hexadecimal string at most.
            ///
            /// For example, if `x` is `H16` (a 16 bits binary data):
            /// - when `x = [0, 0]`, the trimmed hexadecimal string should be "0" or "".
            /// - when `x = [0, 1]`, the trimmed hexadecimal string should be "1".
            /// - when `x = [1, 0]`, the trimmed hexadecimal string should be "100".
            ///
            /// ```rust
            #[doc = $use_stmt]
            #[doc = $bytes_size_stmt]
            ///
            /// let mut inner = [0u8; BYTES_SIZE];
            ///
            /// {
            ///     let actual = Hash(inner.clone());
            ///     let expected1 = Hash::from_trimmed_str("").unwrap();
            ///     let expected2 = Hash::from_trimmed_str("0").unwrap();
            ///     assert_eq!(actual, expected1);
            ///     assert_eq!(actual, expected2);
            /// }
            ///
            /// {
            ///     inner[BYTES_SIZE - 1] = 1;
            ///     let actual = Hash(inner);
            ///     let expected = Hash::from_trimmed_str("1").unwrap();
            ///     assert_eq!(actual, expected);
            /// }
            ///
            /// {
            ///     assert!(Hash::from_trimmed_str("00").is_err());
            ///     assert!(Hash::from_trimmed_str("000").is_err());
            ///     assert!(Hash::from_trimmed_str("0000").is_err());
            ///     assert!(Hash::from_trimmed_str("01").is_err());
            ///     assert!(Hash::from_trimmed_str("001").is_err());
            ///     assert!(Hash::from_trimmed_str("0001").is_err());
            /// }
            /// ```
            pub fn from_trimmed_str(input: &str) -> Result<Self, FromStrError> {
                let bytes = input.as_bytes();
                let len = bytes.len();
                if len > $bytes_size * 2 {
                    Err(FromStrError::InvalidLength(len))
                } else if len == 0 {
                    Ok(Self::default())
                } else if bytes[0] == b'0' {
                    if len == 1 {
                        Ok(Self::default())
                    } else {
                        Err(FromStrError::InvalidCharacter { chr: b'0', idx: 0 })
                    }
                } else {
                    let mut ret = Self::default();
                    let mut idx = 0;
                    let mut unit_idx = ($bytes_size * 2 - len) / 2;
                    let mut high = len % 2 == 0;
                    for chr in input.bytes() {
                        let val = if high {
                            DICT_HEX_HI[usize::from(chr)]
                        } else {
                            DICT_HEX_LO[usize::from(chr)]
                        };
                        if val == DICT_HEX_ERROR {
                            return Err(FromStrError::InvalidCharacter { chr, idx });
                        }
                        idx += 1;
                        ret.0[unit_idx] |= val;
                        if high {
                            high = false;
                        } else {
                            high = true;
                            unit_idx += 1;
                        }
                    }
                    Ok(ret)
                }
            }
        }
    };
    ($name:ident, $bytes_size:expr) => {
        impl_from_trimmed_str!(
            $name,
            $bytes_size,
            concat!("use ckb_fixed_hash_core::", stringify!($name), " as Hash;"),
            concat!("const BYTES_SIZE: usize = ", stringify!($bytes_size), ";")
        );
    };
}

impl_std_str_fromstr!(H160, 20);
impl_std_str_fromstr!(H256, 32);
impl_std_str_fromstr!(H512, 64);
impl_std_str_fromstr!(H520, 65);

impl_from_trimmed_str!(H160, 20);
impl_from_trimmed_str!(H256, 32);
impl_from_trimmed_str!(H512, 64);
impl_from_trimmed_str!(H520, 65);
