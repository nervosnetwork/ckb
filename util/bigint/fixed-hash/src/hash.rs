// Copyright 2015-2017 Parity Technologies
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

/// Return `s` without the `0x` at the beginning of it, if any.
pub fn clean_0x(s: &str) -> &str {
    if s.starts_with("0x") {
        &s[2..]
    } else {
        s
    }
}

#[macro_export]
macro_rules! construct_hash {
    ($from: ident, $size: expr) => {
        #[repr(C)]
        /// Unformatted binary data of fixed length.
        pub struct $from (pub [u8; $size]);


        impl From<[u8; $size]> for $from {
            fn from(bytes: [u8; $size]) -> Self {
                $from(bytes)
            }
        }

        impl From<$from> for [u8; $size] {
            fn from(s: $from) -> Self {
                s.0
            }
        }

        impl ::core::ops::Deref for $from {
            type Target = [u8];

            #[inline]
            fn deref(&self) -> &[u8] {
                &self.0
            }
        }

        impl AsRef<[u8]> for $from {
            #[inline]
            fn as_ref(&self) -> &[u8] {
                &self.0
            }
        }

        impl ::core::ops::DerefMut for $from {
            #[inline]
            fn deref_mut(&mut self) -> &mut [u8] {
                &mut self.0
            }
        }

        impl $from {
            /// Create a new, zero-initialised, instance.
            pub fn new() -> $from {
                $from([0; $size])
            }

            /// Synonym for `new()`. Prefer to new as it's more readable.
            pub fn zero() -> $from {
                $from([0; $size])
            }

            /// Get the size of this object in bytes.
            pub fn len() -> usize {
                $size
            }

            #[inline]
            /// Assign self to be of the same value as a slice of bytes of length `len()`.
            pub fn clone_from_slice(&mut self, src: &[u8]) -> usize {
                let min = ::core::cmp::min($size, src.len());
                self.0[..min].copy_from_slice(&src[..min]);
                min
            }

            /// Convert a slice of bytes of length `len()` to an instance of this type.
            pub fn from_slice(src: &[u8]) -> Self {
                let mut r = Self::new();
                r.clone_from_slice(src);
                r
            }

            /// Copy the data of this object into some mutable slice of length `len()`.
            pub fn copy_to(&self, dest: &mut[u8]) {
                let min = ::core::cmp::min($size, dest.len());
                dest[..min].copy_from_slice(&self.0[..min]);
            }

            /// Returns `true` if all bits set in `b` are also set in `self`.
            pub fn contains<'a>(&'a self, b: &'a Self) -> bool {
                &(b & self) == b
            }

            /// Returns `true` if no bits are set.
            pub fn is_zero(&self) -> bool {
                self.eq(&Self::new())
            }

            /// Returns the lowest 8 bytes interpreted as a BigEndian integer.
            pub fn low_u64(&self) -> u64 {
                let mut ret = 0u64;
                for i in 0..::core::cmp::min($size, 8) {
                    ret |= (self.0[$size - 1 - i] as u64) << (i * 8);
                }
                ret
            }

            impl_std_for_hash_internals!($from, $size);
        }

        impl ::core::fmt::Debug for $from {
            fn fmt(&self, f: &mut ::core::fmt::Formatter) -> ::core::fmt::Result {
                for i in &self.0[..] {
                    write!(f, "{:02x}", i)?;
                }
                Ok(())
            }
        }

        impl ::core::fmt::Display for $from {
            fn fmt(&self, f: &mut ::core::fmt::Formatter) -> ::core::fmt::Result {
                for i in &self.0[0..2] {
                    write!(f, "{:02x}", i)?;
                }
                write!(f, "…")?;
                for i in &self.0[$size - 2..$size] {
                    write!(f, "{:02x}", i)?;
                }
                Ok(())
            }
        }

        impl Copy for $from {}
        #[cfg_attr(feature="dev", allow(expl_impl_clone_on_copy))]
        impl Clone for $from {
            fn clone(&self) -> $from {
                let mut ret = $from::new();
                ret.0.copy_from_slice(&self.0);
                ret
            }
        }

        impl Eq for $from {}

        impl PartialEq for $from {
            fn eq(&self, other: &Self) -> bool {
                unsafe {
                    $crate::libc::memcmp(
                        self.0.as_ptr() as *const $crate::libc::c_void,
                        other.0.as_ptr() as *const $crate::libc::c_void,
                        $size
                    ) == 0
                }
            }
        }

        impl Ord for $from {
            fn cmp(&self, other: &Self) -> ::core::cmp::Ordering {
                let r = unsafe { $crate::libc::memcmp(self.0.as_ptr() as *const $crate::libc::c_void,
                                 other.0.as_ptr() as *const $crate::libc::c_void, $size) };
                if r < 0 { return ::core::cmp::Ordering::Less }
                if r > 0 { return ::core::cmp::Ordering::Greater }
                return ::core::cmp::Ordering::Equal;
            }
        }

        impl PartialOrd for $from {
            fn partial_cmp(&self, other: &Self) -> Option<::core::cmp::Ordering> {
                Some(self.cmp(other))
            }
        }

        impl ::core::hash::Hash for $from {
            fn hash<H>(&self, state: &mut H) where H: ::core::hash::Hasher {
                state.write(&self.0);
                state.finish();
            }
        }

        impl ::core::ops::Index<usize> for $from {
            type Output = u8;

            fn index(&self, index: usize) -> &u8 {
                &self.0[index]
            }
        }
        impl ::core::ops::IndexMut<usize> for $from {
            fn index_mut(&mut self, index: usize) -> &mut u8 {
                &mut self.0[index]
            }
        }
        impl ::core::ops::Index<::core::ops::Range<usize>> for $from {
            type Output = [u8];

            fn index(&self, index: ::core::ops::Range<usize>) -> &[u8] {
                &self.0[index]
            }
        }
        impl ::core::ops::IndexMut<::core::ops::Range<usize>> for $from {
            fn index_mut(&mut self, index: ::core::ops::Range<usize>) -> &mut [u8] {
                &mut self.0[index]
            }
        }
        impl ::core::ops::Index<::core::ops::RangeFull> for $from {
            type Output = [u8];

            fn index(&self, _index: ::core::ops::RangeFull) -> &[u8] {
                &self.0
            }
        }
        impl ::core::ops::IndexMut<::core::ops::RangeFull> for $from {
            fn index_mut(&mut self, _index: ::core::ops::RangeFull) -> &mut [u8] {
                &mut self.0
            }
        }

        /// `BitOr` on references
        impl<'a> ::core::ops::BitOr for &'a $from {
            type Output = $from;

            fn bitor(self, rhs: Self) -> Self::Output {
                let mut ret: $from = $from::default();
                for i in 0..$size {
                    ret.0[i] = self.0[i] | rhs.0[i];
                }
                ret
            }
        }

        /// Moving `BitOr`
        impl ::core::ops::BitOr for $from {
            type Output = $from;

            fn bitor(self, rhs: Self) -> Self::Output {
                &self | &rhs
            }
        }

        /// `BitAnd` on references
        impl <'a> ::core::ops::BitAnd for &'a $from {
            type Output = $from;

            fn bitand(self, rhs: Self) -> Self::Output {
                let mut ret: $from = $from::default();
                for i in 0..$size {
                    ret.0[i] = self.0[i] & rhs.0[i];
                }
                ret
            }
        }

        /// Moving `BitAnd`
        impl ::core::ops::BitAnd for $from {
            type Output = $from;

            fn bitand(self, rhs: Self) -> Self::Output {
                &self & &rhs
            }
        }

        /// `BitXor` on references
        impl <'a> ::core::ops::BitXor for &'a $from {
            type Output = $from;

            fn bitxor(self, rhs: Self) -> Self::Output {
                let mut ret: $from = $from::default();
                for i in 0..$size {
                    ret.0[i] = self.0[i] ^ rhs.0[i];
                }
                ret
            }
        }

        /// Moving `BitXor`
        impl ::core::ops::BitXor for $from {
            type Output = $from;

            fn bitxor(self, rhs: Self) -> Self::Output {
                &self ^ &rhs
            }
        }

        impl Default for $from {
            fn default() -> Self { $from::new() }
        }

        impl From<u64> for $from {
            fn from(mut value: u64) -> $from {
                let mut ret = $from::new();
                for i in 0..8 {
                    if i < $size {
                        ret.0[$size - i - 1] = (value & 0xff) as u8;
                        value >>= 8;
                    }
                }
                ret
            }
        }

        impl<'a> From<&'a [u8]> for $from {
            fn from(s: &'a [u8]) -> $from {
                $from::from_slice(s)
            }
        }

        impl_std_for_hash!($from, $size);
        impl_heapsize_for_hash!($from);
    }
}

#[cfg(feature = "heapsizeof")]
#[macro_export]
#[doc(hidden)]
macro_rules! impl_heapsize_for_hash {
    ($name: ident) => {
        impl $crate::heapsize::HeapSizeOf for $name {
            fn heap_size_of_children(&self) -> usize {
                0
            }
        }
    }
}

#[cfg(not(feature = "heapsizeof"))]
#[macro_export]
#[doc(hidden)]
macro_rules! impl_heapsize_for_hash {
    ($name: ident) => {}
}

#[cfg(feature = "std")]
#[macro_export]
#[doc(hidden)]
macro_rules! impl_std_for_hash {
    ($from: ident, $size: tt) => {
        impl $from {
            /// Get a hex representation.
            pub fn hex(&self) -> String {
                format!("{:?}", self)
            }
        }

        impl $crate::rand::Rand for $from {
            fn rand<R: $crate::rand::Rng>(r: &mut R) -> Self {
                let mut hash = $from::new();
                r.fill_bytes(&mut hash.0);
                hash
            }
        }

        impl ::core::str::FromStr for $from {
            type Err = $crate::rustc_hex::FromHexError;

            fn from_str(s: &str) -> Result<$from, $crate::rustc_hex::FromHexError> {
                use $crate::rustc_hex::FromHex;
                let a = s.from_hex()?;
                if a.len() != $size {
                    return Err($crate::rustc_hex::FromHexError::InvalidHexLength);
                }

                let mut ret = [0; $size];
                ret.copy_from_slice(&a);
                Ok($from(ret))
            }
        }

        impl From<&'static str> for $from {
            fn from(s: &'static str) -> $from {
                let s = $crate::clean_0x(s);
                if s.len() % 2 == 1 {
                    ("0".to_owned() + s).parse().unwrap()
                } else {
                    s.parse().unwrap()
                }
            }
        }
    }
}

#[cfg(not(feature = "std"))]
#[macro_export]
#[doc(hidden)]
macro_rules! impl_std_for_hash {
    ($from: ident, $size: tt) => {}
}

#[cfg(feature = "std")]
#[macro_export]
#[doc(hidden)]
macro_rules! impl_std_for_hash_internals {
    ($from: ident, $size: tt) => {
        /// Create a new, cryptographically random, instance.
        pub fn random() -> $from {
            let mut hash = $from::new();
            hash.randomize();
            hash
        }

        /// Assign self have a cryptographically random value.
        pub fn randomize(&mut self) {
            let mut rng = $crate::rand::OsRng::new().unwrap();
            *self = $crate::rand::Rand::rand(&mut rng);
        }
    }
}

#[cfg(not(feature = "std"))]
#[macro_export]
#[doc(hidden)]
macro_rules! impl_std_for_hash_internals {
    ($from: ident, $size: tt) => {}
}
