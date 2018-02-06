// Copyright 2015-2017 Parity Technologies
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

// Code derived from original work by Andrew Poelstra <apoelstra@wpsoftware.net>

// Rust Bitcoin Library
// Written in 2014 by
//     Andrew Poelstra <apoelstra@wpsoftware.net>
//
// To the extent possible under law, the author(s) have dedicated all
// copyright and related and neighboring rights to this software to
// the public domain worldwide. This software is distributed without
// any warranty.
//
// You should have received a copy of the CC0 Public Domain Dedication
// along with this software.
// If not, see <http://creativecommons.org/publicdomain/zero/1.0/>.
//

//! Big unsigned integer types.
//!
//! Implementation of a various large-but-fixed sized unsigned integer types.
//! The functions here are designed to be fast. There are optional `x86_64`
//! implementations for even more speed, hidden behind the `x64_arithmetic`
//! feature flag.

/// Conversion from decimal string error
#[derive(Debug, PartialEq)]
pub enum FromDecStrErr {
    /// Char not from range 0-9
    InvalidCharacter,
    /// Value does not fit into type
    InvalidLength,
}

#[macro_export]
#[doc(hidden)]
macro_rules! impl_map_from {
    ($thing:ident, $from:ty, $to:ty) => {
        impl From<$from> for $thing {
            fn from(value: $from) -> $thing {
                From::from(value as $to)
            }
        }
    }
}

#[cfg(not(all(asm_available, target_arch = "x86_64")))]
#[macro_export]
#[doc(hidden)]
macro_rules! uint_overflowing_add {
    ($name:ident, $n_words:tt, $self_expr: expr, $other: expr) => ({
        uint_overflowing_add_reg!($name, $n_words, $self_expr, $other)
    })
}

#[macro_export]
#[doc(hidden)]
macro_rules! uint_overflowing_add_reg {
    ($name:ident, $n_words:tt, $self_expr: expr, $other: expr) => ({
        uint_overflowing_binop!(
            $name,
            $n_words,
            $self_expr,
            $other,
            u64::overflowing_add
        )
    })
}

#[cfg(all(asm_available, target_arch = "x86_64"))]
#[macro_export]
#[doc(hidden)]
macro_rules! uint_overflowing_add {
    (U256, $n_words:tt, $self_expr: expr, $other: expr) => ({
        let mut result: [u64; $n_words] = unsafe { ::core::mem::uninitialized() };
        let self_t: &[u64; $n_words] = &$self_expr.0;
        let other_t: &[u64; $n_words] = &$other.0;

        let overflow: u8;
        unsafe {
            asm!("
                add $9, $0
                adc $10, $1
                adc $11, $2
                adc $12, $3
                setc %al
                "
            : "=r"(result[0]), "=r"(result[1]), "=r"(result[2]), "=r"(result[3]), "={al}"(overflow)
            : "0"(self_t[0]), "1"(self_t[1]), "2"(self_t[2]), "3"(self_t[3]),
              "mr"(other_t[0]), "mr"(other_t[1]), "mr"(other_t[2]), "mr"(other_t[3])
            :
            :
            );
        }
        (U256(result), overflow != 0)
    });
    (U512, $n_words:tt, $self_expr: expr, $other: expr) => ({
        let mut result: [u64; $n_words] = unsafe { ::core::mem::uninitialized() };
        let self_t: &[u64; $n_words] = &$self_expr.0;
        let other_t: &[u64; $n_words] = &$other.0;

        let overflow: u8;

        unsafe {
            asm!("
                add $15, $0
                adc $16, $1
                adc $17, $2
                adc $18, $3
                lodsq
                adc $11, %rax
                stosq
                lodsq
                adc $12, %rax
                stosq
                lodsq
                adc $13, %rax
                stosq
                lodsq
                adc $14, %rax
                stosq
                setc %al

                ": "=r"(result[0]), "=r"(result[1]), "=r"(result[2]), "=r"(result[3]),

              "={al}"(overflow) /* $0 - $4 */

            : "{rdi}"(&result[4] as *const u64) /* $5 */
              "{rsi}"(&other_t[4] as *const u64) /* $6 */
              "0"(self_t[0]), "1"(self_t[1]), "2"(self_t[2]), "3"(self_t[3]),
                "m"(self_t[4]), "m"(self_t[5]), "m"(self_t[6]), "m"(self_t[7]),
              /* $7 - $14 */

              "mr"(other_t[0]), "mr"(other_t[1]), "mr"(other_t[2]), "mr"(other_t[3]),
              "m"(other_t[4]), "m"(other_t[5]), "m"(other_t[6]), "m"(other_t[7]) /* $15 - $22 */
            : "rdi", "rsi"
            :
            );
        }
        (U512(result), overflow != 0)
    });

    ($name:ident, $n_words:tt, $self_expr: expr, $other: expr) => (
        uint_overflowing_add_reg!($name, $n_words, $self_expr, $other)
    )
}

#[cfg(not(all(asm_available, target_arch = "x86_64")))]
#[macro_export]
#[doc(hidden)]
macro_rules! uint_overflowing_sub {
    ($name:ident, $n_words:tt, $self_expr: expr, $other: expr) => ({
        uint_overflowing_sub_reg!($name, $n_words, $self_expr, $other)
    })
}

#[macro_export]
#[doc(hidden)]
macro_rules! uint_overflowing_binop {
    ($name:ident, $n_words:tt, $self_expr: expr, $other: expr, $fn:expr) => ({
        let $name(ref me) = $self_expr;
        let $name(ref you) = $other;

        let mut ret = unsafe { ::core::mem::uninitialized() };
        let ret_ptr = &mut ret as *mut [u64; $n_words] as *mut u64;
        let mut carry = 0u64;

        unroll! {
            for i in 0..$n_words {
                use ::core::ptr;

                if carry != 0 {
                    let (res1, overflow1) = ($fn)(me[i], you[i]);
                    let (res2, overflow2) = ($fn)(res1, carry);

                    unsafe {
                        ptr::write(
                            ret_ptr.offset(i as _),
                            res2
                        );
                    }
                    carry = (overflow1 as u8 + overflow2 as u8) as u64;
                } else {
                    let (res, overflow) = ($fn)(me[i], you[i]);

                    unsafe {
                        ptr::write(
                            ret_ptr.offset(i as _),
                            res
                        );
                    }

                    carry = overflow as u64;
                }
            }
        }

        ($name(ret), carry > 0)
    })
}

#[macro_export]
#[doc(hidden)]
macro_rules! uint_overflowing_sub_reg {
    ($name:ident, $n_words:tt, $self_expr: expr, $other: expr) => ({
        uint_overflowing_binop!(
            $name,
            $n_words,
            $self_expr,
            $other,
            u64::overflowing_sub
        )
    })
}

#[cfg(all(asm_available, target_arch = "x86_64"))]
#[macro_export]
#[doc(hidden)]
macro_rules! uint_overflowing_sub {
    (U256, $n_words:tt, $self_expr: expr, $other: expr) => ({
        let mut result: [u64; $n_words] = unsafe { ::core::mem::uninitialized() };
        let self_t: &[u64; $n_words] = &$self_expr.0;
        let other_t: &[u64; $n_words] = &$other.0;

        let overflow: u8;
        unsafe {
            asm!("
                sub $9, $0
                sbb $10, $1
                sbb $11, $2
                sbb $12, $3
                setb %al
                "
                : "=r"(result[0]), "=r"(result[1]), "=r"(result[2]), "=r"(result[3]), "={al}"(overflow)
                : "0"(self_t[0]), "1"(self_t[1]), "2"(self_t[2]), "3"(self_t[3]), "mr"(other_t[0]), "mr"(other_t[1]), "mr"(other_t[2]), "mr"(other_t[3])
                :
                :
            );
        }
        (U256(result), overflow != 0)
    });
    (U512, $n_words:tt, $self_expr: expr, $other: expr) => ({
        let mut result: [u64; $n_words] = unsafe { ::core::mem::uninitialized() };
        let self_t: &[u64; $n_words] = &$self_expr.0;
        let other_t: &[u64; $n_words] = &$other.0;

        let overflow: u8;

        unsafe {
            asm!("
                sub $15, $0
                sbb $16, $1
                sbb $17, $2
                sbb $18, $3
                lodsq
                sbb $19, %rax
                stosq
                lodsq
                sbb $20, %rax
                stosq
                lodsq
                sbb $21, %rax
                stosq
                lodsq
                sbb $22, %rax
                stosq
                setb %al
                "
            : "=r"(result[0]), "=r"(result[1]), "=r"(result[2]), "=r"(result[3]),

              "={al}"(overflow) /* $0 - $4 */

            : "{rdi}"(&result[4] as *const u64) /* $5 */
             "{rsi}"(&self_t[4] as *const u64) /* $6 */
              "0"(self_t[0]), "1"(self_t[1]), "2"(self_t[2]), "3"(self_t[3]),
              "m"(self_t[4]), "m"(self_t[5]), "m"(self_t[6]), "m"(self_t[7]),
              /* $7 - $14 */

              "m"(other_t[0]), "m"(other_t[1]), "m"(other_t[2]), "m"(other_t[3]),
              "m"(other_t[4]), "m"(other_t[5]), "m"(other_t[6]), "m"(other_t[7]) /* $15 - $22 */
            : "rdi", "rsi"
            :
            );
        }
        (U512(result), overflow != 0)
    });
    ($name:ident, $n_words:tt, $self_expr: expr, $other: expr) => ({
        uint_overflowing_sub_reg!($name, $n_words, $self_expr, $other)
    })
}

#[cfg(all(asm_available, target_arch = "x86_64"))]
#[macro_export]
macro_rules! uint_overflowing_mul {
    (U256, $n_words: expr, $self_expr: expr, $other: expr) => ({
        let mut result: [u64; $n_words] = unsafe { ::core::mem::uninitialized() };
        let self_t: &[u64; $n_words] = &$self_expr.0;
        let other_t: &[u64; $n_words] = &$other.0;

        let overflow: u64;
        unsafe {
            asm!("
                mov $5, %rax
                mulq $9
                mov %rax, $0
                mov %rdx, $1

                mov $5, %rax
                mulq $10
                add %rax, $1
                adc $$0, %rdx
                mov %rdx, $2

                mov $5, %rax
                mulq $11
                add %rax, $2
                adc $$0, %rdx
                mov %rdx, $3

                mov $5, %rax
                mulq $12
                add %rax, $3
                adc $$0, %rdx
                mov %rdx, %rcx

                mov $6, %rax
                mulq $9
                add %rax, $1
                adc %rdx, $2
                adc $$0, $3
                adc $$0, %rcx

                mov $6, %rax
                mulq $10
                add %rax, $2
                adc %rdx, $3
                adc $$0, %rcx
                adc $$0, $3
                adc $$0, %rcx

                mov $6, %rax
                mulq $11
                add %rax, $3
                adc $$0, %rdx
                or %rdx, %rcx

                mov $7, %rax
                mulq $9
                add %rax, $2
                adc %rdx, $3
                adc $$0, %rcx

                mov $7, %rax
                mulq $10
                add %rax, $3
                adc $$0, %rdx
                or %rdx, %rcx

                mov $8, %rax
                mulq $9
                add %rax, $3
                or %rdx, %rcx

                cmpq $$0, %rcx
                jne 2f

                mov $8, %rcx
                jrcxz 12f

                mov $12, %rcx
                mov $11, %rax
                or %rax, %rcx
                mov $10, %rax
                or %rax, %rcx
                jmp 2f

                12:
                mov $12, %rcx
                jrcxz 11f

                mov $7, %rcx
                mov $6, %rax
                or %rax, %rcx

                cmpq $$0, %rcx
                jne 2f

                11:
                mov $11, %rcx
                jrcxz 2f
                mov $7, %rcx

                2:
                "
                : /* $0 */ "={r8}"(result[0]), /* $1 */ "={r9}"(result[1]), /* $2 */ "={r10}"(result[2]),
                  /* $3 */ "={r11}"(result[3]), /* $4 */  "={rcx}"(overflow)

                : /* $5 */ "m"(self_t[0]), /* $6 */ "m"(self_t[1]), /* $7 */  "m"(self_t[2]),
                  /* $8 */ "m"(self_t[3]), /* $9 */ "m"(other_t[0]), /* $10 */ "m"(other_t[1]),
                  /* $11 */ "m"(other_t[2]), /* $12 */ "m"(other_t[3])
                   : "rax", "rdx"
                :

            );
        }
        (U256(result), overflow > 0)
    });
    ($name:ident, $n_words:tt, $self_expr: expr, $other: expr) => (
        uint_overflowing_mul_reg!($name, $n_words, $self_expr, $other)
    )
}

#[cfg(not(all(asm_available, target_arch = "x86_64")))]
#[macro_export]
#[doc(hidden)]
macro_rules! uint_overflowing_mul {
    ($name:ident, $n_words:tt, $self_expr: expr, $other: expr) => ({
        uint_overflowing_mul_reg!($name, $n_words, $self_expr, $other)
    })
}

#[macro_export]
#[doc(hidden)]
macro_rules! uint_full_mul_reg {
    ($name:ident, $n_words:tt, $self_expr:expr, $other:expr) => ({{
        #![allow(unused_assignments)]

        let $name(ref me) = $self_expr;
        let $name(ref you) = $other;
        let mut ret = [0u64; 2*$n_words];

        unroll! {
            for i in 0..$n_words {
                let mut carry = 0u64;
                let (b_u, b_l) = $crate::split(you[i]);

                unroll! {
                    for j in 0..$n_words {
                        if me[j] != 0 || carry != 0 {
                            let a = $crate::split(me[j]);

                            // multiply parts
                            let (c_l, overflow_l) = $crate::mul_u32(a, b_l, ret[i + j]);
                            let (c_u, overflow_u) = $crate::mul_u32(a, b_u, c_l >> 32);
                            ret[i + j] = (c_l & 0xFFFFFFFF) + (c_u << 32);

                            // No overflow here
                            let res = (c_u >> 32) + (overflow_u << 32);
                            // possible overflows
                            let (res, o1) = res.overflowing_add(overflow_l + carry);
                            let (res, o2) = res.overflowing_add(ret[i + j + 1]);
                            ret[i + j + 1] = res;

                            // Only single overflow possible there
                            carry = (o1 | o2) as u64;
                        }
                    }
                }
            }
        }

        ret
    }})
}

#[macro_export]
#[doc(hidden)]
macro_rules! uint_overflowing_mul_reg {
    ($name:ident, $n_words:tt, $self_expr: expr, $other: expr) => ({
        let ret: [u64; $n_words * 2] = uint_full_mul_reg!($name, $n_words, $self_expr, $other);

        // The safety of this is enforced by the compiler
        let ret: [[u64; $n_words]; 2] = unsafe { ::core::mem::transmute(ret) };

        // The compiler WILL NOT inline this if you remove this annotation.
        #[inline(always)]
        fn any_nonzero(arr: &[u64; $n_words]) -> bool {
            unroll! {
                for i in 0..$n_words {
                    if arr[i] != 0 {
                        return true;
                    }
                }
            }

            false
        }

        ($name(ret[0]), any_nonzero(&ret[1]))
    })
}

#[macro_export]
#[doc(hidden)]
macro_rules! overflowing {
    ($op: expr, $overflow: expr) => (
        {
            let (overflow_x, overflow_overflow) = $op;
            $overflow |= overflow_overflow;
            overflow_x
        }
    );
    ($op: expr) => (
        {
            let (overflow_x, _overflow_overflow) = $op;
            overflow_x
        }
    );
}

#[macro_export]
#[doc(hidden)]
macro_rules! panic_on_overflow {
    ($name: expr) => {
        if $name {
            panic!("arithmetic operation overflow")
        }
    }
}

#[inline(always)]
#[doc(hidden)]
pub fn mul_u32(a: (u64, u64), b: u64, carry: u64) -> (u64, u64) {
    let upper = b * a.0;
    let lower = b * a.1;

    let (res1, overflow1) = lower.overflowing_add(upper << 32);
    let (res2, overflow2) = res1.overflowing_add(carry);

    let carry = (upper >> 32) + overflow1 as u64 + overflow2 as u64;
    (res2, carry)
}

#[inline(always)]
#[doc(hidden)]
pub fn split(a: u64) -> (u64, u64) {
    (a >> 32, a & 0xFFFFFFFF)
}

#[macro_export]
macro_rules! construct_uint {
    ($name:ident, $n_words:tt) => (
        /// Little-endian large integer type
        #[repr(C)]
        #[derive(Copy, Clone, Eq, PartialEq, Hash)]
        pub struct $name(pub [u64; $n_words]);

        impl $name {
            /// Convert from a decimal string.
            pub fn from_dec_str(value: &str) -> Result<Self, $crate::FromDecStrErr> {
                if !value.bytes().all(|b| b >= 48 && b <= 57) {
                    return Err($crate::FromDecStrErr::InvalidCharacter)
                }

                let mut res = Self::default();
                for b in value.bytes().map(|b| b - 48) {
                    let (r, overflow) = res.overflowing_mul_u32(10);
                    if overflow {
                        return Err($crate::FromDecStrErr::InvalidLength);
                    }
                    let (r, overflow) = r.overflowing_add(b.into());
                    if overflow {
                        return Err($crate::FromDecStrErr::InvalidLength);
                    }
                    res = r;
                }
                Ok(res)
            }

            /// Conversion to u32
            #[inline]
            pub fn low_u32(&self) -> u32 {
                let &$name(ref arr) = self;
                arr[0] as u32
            }

            /// Conversion to u64
            #[inline]
            pub fn low_u64(&self) -> u64 {
                let &$name(ref arr) = self;
                arr[0]
            }

            /// Conversion to u32 with overflow checking
            ///
            /// # Panics
            ///
            /// Panics if the number is larger than 2^32.
            #[inline]
            pub fn as_u32(&self) -> u32 {
                let &$name(ref arr) = self;
                if (arr[0] & (0xffffffffu64 << 32)) != 0 {
                    panic!("Integer overflow when casting U256")
                }
                self.as_u64() as u32
            }

            /// Conversion to u64 with overflow checking
            ///
            /// # Panics
            ///
            /// Panics if the number is larger than 2^64.
            #[inline]
            pub fn as_u64(&self) -> u64 {
                let &$name(ref arr) = self;
                for i in 1..$n_words {
                    if arr[i] != 0 {
                        panic!("Integer overflow when casting U256")
                    }
                }
                arr[0]
            }

            /// Whether this is zero.
            #[inline]
            pub fn is_zero(&self) -> bool {
                let &$name(ref arr) = self;
                for i in 0..$n_words { if arr[i] != 0 { return false; } }
                return true;
            }

            /// Return the least number of bits needed to represent the number
            #[inline]
            pub fn bits(&self) -> usize {
                let &$name(ref arr) = self;
                for i in 1..$n_words {
                    if arr[$n_words - i] > 0 {
                        return (0x40 * ($n_words - i + 1))
                               - arr[$n_words - i].leading_zeros() as usize;
                    }
                }
                0x40 - arr[0].leading_zeros() as usize
            }

            /// Return if specific bit is set.
            ///
            /// # Panics
            ///
            /// Panics if `index` exceeds the bit width of the number.
            #[inline]
            pub fn bit(&self, index: usize) -> bool {
                let &$name(ref arr) = self;
                arr[index / 64] & (1 << (index % 64)) != 0
            }

            /// Returns the number of leading zeros in the binary representation of self.
            pub fn leading_zeros(&self) -> u32 {
                let mut r = 0;
                for i in 0..$n_words {
                    let w = self.0[$n_words - i - 1];
                    if w == 0 {
                        r += 64;
                    } else {
                        r += w.leading_zeros();
                        break;
                    }
                }
                r
            }

            /// Returns the number of leading zeros in the binary representation of self.
            pub fn trailing_zeros(&self) -> u32 {
                let mut r = 0;
                for i in 0..$n_words {
                    let w = self.0[i];
                    if w == 0 {
                        r += 64;
                    } else {
                        r += w.trailing_zeros();
                        break;
                    }
                }
                r
            }

            /// Return specific byte.
            ///
            /// # Panics
            ///
            /// Panics if `index` exceeds the byte width of the number.
            #[inline]
            pub fn byte(&self, index: usize) -> u8 {
                let &$name(ref arr) = self;
                (arr[index / 8] >> (((index % 8)) * 8)) as u8
            }

            /// Write to the slice in big-endian format.
            #[inline]
            pub fn to_big_endian(&self, bytes: &mut [u8]) {
                use $crate::byteorder::{ByteOrder, BigEndian};
                debug_assert!($n_words * 8 == bytes.len());
                for i in 0..$n_words {
                    BigEndian::write_u64(&mut bytes[8 * i..], self.0[$n_words - i - 1]);
                }
            }

            /// Write to the slice in little-endian format.
            #[inline]
            pub fn to_little_endian(&self, bytes: &mut [u8]) {
                use $crate::byteorder::{ByteOrder, LittleEndian};
                debug_assert!($n_words * 8 == bytes.len());
                for i in 0..$n_words {
                    LittleEndian::write_u64(&mut bytes[8 * i..], self.0[i]);
                }
            }


            /// Create `10**n` as this type.
            ///
            /// # Panics
            ///
            /// Panics if the result overflows the type.
            #[inline]
            pub fn exp10(n: usize) -> Self {
                match n {
                    0 => Self::from(1u64),
                    _ => Self::exp10(n - 1).mul_u32(10)
                }
            }

            /// Zero (additive identity) of this type.
            #[inline]
            pub fn zero() -> Self {
                From::from(0u64)
            }

            /// One (multiplicative identity) of this type.
            #[inline]
            pub fn one() -> Self {
                From::from(1u64)
            }

            /// The maximum value which can be inhabited by this type.
            #[inline]
            pub fn max_value() -> Self {
                let mut result = [0; $n_words];
                for i in 0..$n_words {
                    result[i] = u64::max_value();
                }
                $name(result)
            }

            /// Fast exponentation by squaring
            /// https://en.wikipedia.org/wiki/Exponentiation_by_squaring
            ///
            /// # Panics
            ///
            /// Panics if the result overflows the type.
            pub fn pow(self, expon: Self) -> Self {
                if expon.is_zero() {
                    return Self::one()
                }
                let is_even = |x : &Self| x.low_u64() & 1 == 0;

                let u_one = Self::one();
                let mut y = u_one;
                let mut n = expon;
                let mut x = self;
                while n > u_one {
                    if is_even(&n) {
                        x = x * x;
                        n = n >> 1;
                    } else {
                        y = x * y;
                        x = x * x;
                        // to reduce odd number by 1 we should just clear the last bit
                        n.0[$n_words-1] = n.0[$n_words-1] & ((!0u64)>>1);
                        n = n >> 1;
                    }
                }
                x * y
            }

            /// Fast exponentation by squaring
            /// https://en.wikipedia.org/wiki/Exponentiation_by_squaring
            pub fn overflowing_pow(self, expon: Self) -> (Self, bool) {
                if expon.is_zero() { return (Self::one(), false) }

                let is_even = |x : &Self| x.low_u64() & 1 == 0;

                let u_one = Self::one();
                let mut y = u_one;
                let mut n = expon;
                let mut x = self;
                let mut overflow = false;

                while n > u_one {
                    if is_even(&n) {
                        x = overflowing!(x.overflowing_mul(x), overflow);
                        n = n >> 1;
                    } else {
                        y = overflowing!(x.overflowing_mul(y), overflow);
                        x = overflowing!(x.overflowing_mul(x), overflow);
                        n = (n - u_one) >> 1;
                    }
                }
                let res = overflowing!(x.overflowing_mul(y), overflow);
                (res, overflow)
            }

            /// Optimized instructions
            #[inline(always)]
            pub fn overflowing_add(self, other: $name) -> ($name, bool) {
                uint_overflowing_add!($name, $n_words, self, other)
            }

            /// Addition which saturates at the maximum value.
            pub fn saturating_add(self, other: $name) -> $name {
                match self.overflowing_add(other) {
                    (_, true) => $name::max_value(),
                    (val, false) => val,
                }
            }

            /// Checked addition. Returns `None` if overflow occurred.
            pub fn checked_add(self, other: $name) -> Option<$name> {
                match self.overflowing_add(other) {
                    (_, true) => None,
                    (val, _) => Some(val),
                }
            }

            /// Subtraction which underflows and returns a flag if it does.
            #[inline(always)]
            pub fn overflowing_sub(self, other: $name) -> ($name, bool) {
                uint_overflowing_sub!($name, $n_words, self, other)
            }

            /// Subtraction which saturates at zero.
            pub fn saturating_sub(self, other: $name) -> $name {
                match self.overflowing_sub(other) {
                    (_, true) => $name::zero(),
                    (val, false) => val,
                }
            }

            /// Checked subtraction. Returns `None` if overflow occurred.
            pub fn checked_sub(self, other: $name) -> Option<$name> {
                match self.overflowing_sub(other) {
                    (_, true) => None,
                    (val, _) => Some(val),
                }
            }

            /// Multiply with overflow, returning a flag if it does.
            #[inline(always)]
            pub fn overflowing_mul(self, other: $name) -> ($name, bool) {
                uint_overflowing_mul!($name, $n_words, self, other)
            }

            /// Multiplication which saturates at the maximum value..
            pub fn saturating_mul(self, other: $name) -> $name {
                match self.overflowing_mul(other) {
                    (_, true) => $name::max_value(),
                    (val, false) => val,
                }
            }

            /// Checked multiplication. Returns `None` if overflow occurred.
            pub fn checked_mul(self, other: $name) -> Option<$name> {
                match self.overflowing_mul(other) {
                    (_, true) => None,
                    (val, _) => Some(val),
                }
            }

            /// Division with overflow
            pub fn overflowing_div(self, other: $name) -> ($name, bool) {
                (self / other, false)
            }

            /// Checked division. Returns `None` if `other == 0`.
            pub fn checked_div(self, other: $name) -> Option<$name> {
                if other.is_zero() {
                    None
                } else {
                    Some(self / other)
                }
            }

            /// Modulus with overflow.
            pub fn overflowing_rem(self, other: $name) -> ($name, bool) {
                (self % other, false)
            }

            /// Checked modulus. Returns `None` if `other == 0`.
            pub fn checked_rem(self, other: $name) -> Option<$name> {
                if other.is_zero() {
                    None
                } else {
                    Some(self % other)
                }
            }

            /// Negation with overflow.
            pub fn overflowing_neg(self) -> ($name, bool) {
                if self.is_zero() {
                    (self, false)
                } else {
                    (!self, true)
                }
            }

            /// Checked negation. Returns `None` unless `self == 0`.
            pub fn checked_neg(self) -> Option<$name> {
                match self.overflowing_neg() {
                    (_, true) => None,
                    (zero, false) => Some(zero),
                }
            }

            /// Multiplication by u32
            pub fn mul_u32(self, other: u32) -> Self {
                let (ret, overflow) = self.overflowing_mul_u32(other);
                panic_on_overflow!(overflow);
                ret
            }

            /// Overflowing multiplication by u32
            #[allow(dead_code)] // not used when multiplied with inline assembly
            fn overflowing_mul_u32(self, other: u32) -> (Self, bool) {
                let $name(ref arr) = self;
                let mut ret = [0u64; $n_words];
                let mut carry = 0;
                let o = other as u64;

                for i in 0..$n_words {
                    let (res, carry2) = $crate::mul_u32($crate::split(arr[i]), o, carry);
                    ret[i] = res;
                    carry = carry2;
                }

                ($name(ret), carry > 0)
            }

            /// Converts from big endian representation bytes in memory
            /// Can also be used as (&slice).into(), as it is default `From`
            /// slice implementation for U256
            pub fn from_big_endian(slice: &[u8]) -> Self {
                assert!($n_words * 8 >= slice.len());

                let mut ret = [0; $n_words];
                unsafe {
                    let ret_u8: &mut [u8; $n_words * 8] = ::core::mem::transmute(&mut ret);
                    let mut ret_ptr = ret_u8.as_mut_ptr();
                    let mut slice_ptr = slice.as_ptr().offset(slice.len() as isize - 1);
                    for _ in 0..slice.len() {
                        *ret_ptr = *slice_ptr;
                        ret_ptr = ret_ptr.offset(1);
                        slice_ptr = slice_ptr.offset(-1);
                    }
                }

                $name(ret)
            }

            /// Converts from little endian representation bytes in memory
            pub fn from_little_endian(slice: &[u8]) -> Self {
                assert!($n_words * 8 >= slice.len());

                let mut ret = [0; $n_words];
                unsafe {
                    let ret_u8: &mut [u8; $n_words * 8] = ::core::mem::transmute(&mut ret);
                    ret_u8[0..slice.len()].copy_from_slice(&slice);
                }

                $name(ret)
            }

            impl_std_for_uint_internals!($name, $n_words);
        }

        impl Default for $name {
            fn default() -> Self {
                $name::zero()
            }
        }

        impl From<u64> for $name {
            fn from(value: u64) -> $name {
                let mut ret = [0; $n_words];
                ret[0] = value;
                $name(ret)
            }
        }


        impl_map_from!($name, u8, u64);
        impl_map_from!($name, u16, u64);
        impl_map_from!($name, u32, u64);
        impl_map_from!($name, usize, u64);

        impl From<i64> for $name {
            fn from(value: i64) -> $name {
                match value >= 0 {
                    true => From::from(value as u64),
                    false => { panic!("Unsigned integer can't be created from negative value"); }
                }
            }
        }

        impl_map_from!($name, i8, i64);
        impl_map_from!($name, i16, i64);
        impl_map_from!($name, i32, i64);
        impl_map_from!($name, isize, i64);

        // Converts from big endian representation of U256
        impl<'a> From<&'a [u8]> for $name {
            fn from(bytes: &[u8]) -> $name {
                Self::from_big_endian(bytes)
            }
        }


        impl ::core::ops::Add<$name> for $name {
            type Output = $name;

            fn add(self, other: $name) -> $name {
                let (result, overflow) = self.overflowing_add(other);
                panic_on_overflow!(overflow);
                result
            }
        }

        impl ::core::ops::Sub<$name> for $name {
            type Output = $name;

            #[inline]
            fn sub(self, other: $name) -> $name {
                let (result, overflow) = self.overflowing_sub(other);
                panic_on_overflow!(overflow);
                result
            }
        }

        impl ::core::ops::Mul<$name> for $name {
            type Output = $name;

            fn mul(self, other: $name) -> $name {
                let (result, overflow) = self.overflowing_mul(other);
                panic_on_overflow!(overflow);
                result
            }
        }

        impl ::core::ops::Div<$name> for $name {
            type Output = $name;

            fn div(self, other: $name) -> $name {
                let mut sub_copy = self;
                let mut shift_copy = other;
                let mut ret = [0u64; $n_words];

                let my_bits = self.bits();
                let your_bits = other.bits();

                // Check for division by 0
                assert!(your_bits != 0);

                // Early return in case we are dividing by a larger number than us
                if my_bits < your_bits {
                    return $name(ret);
                }

                // Bitwise long division
                let mut shift = my_bits - your_bits;
                shift_copy = shift_copy << shift;
                loop {
                    if sub_copy >= shift_copy {
                        ret[shift / 64] |= 1 << (shift % 64);
                        sub_copy = overflowing!(sub_copy.overflowing_sub(shift_copy));
                    }
                    shift_copy = shift_copy >> 1;
                    if shift == 0 { break; }
                    shift -= 1;
                }

                $name(ret)
            }
        }

        impl ::core::ops::Rem<$name> for $name {
            type Output = $name;

            fn rem(self, other: $name) -> $name {
                let times = self / other;
                self - (times * other)
            }
        }

        impl ::core::ops::BitAnd<$name> for $name {
            type Output = $name;

            #[inline]
            fn bitand(self, other: $name) -> $name {
                let $name(ref arr1) = self;
                let $name(ref arr2) = other;
                let mut ret = [0u64; $n_words];
                for i in 0..$n_words {
                    ret[i] = arr1[i] & arr2[i];
                }
                $name(ret)
            }
        }

        impl ::core::ops::BitXor<$name> for $name {
            type Output = $name;

            #[inline]
            fn bitxor(self, other: $name) -> $name {
                let $name(ref arr1) = self;
                let $name(ref arr2) = other;
                let mut ret = [0u64; $n_words];
                for i in 0..$n_words {
                    ret[i] = arr1[i] ^ arr2[i];
                }
                $name(ret)
            }
        }

        impl ::core::ops::BitOr<$name> for $name {
            type Output = $name;

            #[inline]
            fn bitor(self, other: $name) -> $name {
                let $name(ref arr1) = self;
                let $name(ref arr2) = other;
                let mut ret = [0u64; $n_words];
                for i in 0..$n_words {
                    ret[i] = arr1[i] | arr2[i];
                }
                $name(ret)
            }
        }

        impl ::core::ops::Not for $name {
            type Output = $name;

            #[inline]
            fn not(self) -> $name {
                let $name(ref arr) = self;
                let mut ret = [0u64; $n_words];
                for i in 0..$n_words {
                    ret[i] = !arr[i];
                }
                $name(ret)
            }
        }

        impl ::core::ops::Shl<usize> for $name {
            type Output = $name;

            fn shl(self, shift: usize) -> $name {
                let $name(ref original) = self;
                let mut ret = [0u64; $n_words];
                let word_shift = shift / 64;
                let bit_shift = shift % 64;

                // shift
                for i in word_shift..$n_words {
                    ret[i] = original[i - word_shift] << bit_shift;
                }
                // carry
                if bit_shift > 0 {
                    for i in word_shift+1..$n_words {
                        ret[i] += original[i - 1 - word_shift] >> (64 - bit_shift);
                    }
                }
                $name(ret)
            }
        }

        impl ::core::ops::Shr<usize> for $name {
            type Output = $name;

            fn shr(self, shift: usize) -> $name {
                let $name(ref original) = self;
                let mut ret = [0u64; $n_words];
                let word_shift = shift / 64;
                let bit_shift = shift % 64;

                // shift
                for i in word_shift..$n_words {
                    ret[i - word_shift] = original[i] >> bit_shift;
                }

                // Carry
                if bit_shift > 0 {
                    for i in word_shift+1..$n_words {
                        ret[i - word_shift - 1] += original[i] << (64 - bit_shift);
                    }
                }

                $name(ret)
            }
        }

        impl Ord for $name {
            fn cmp(&self, other: &$name) -> ::core::cmp::Ordering {
                let &$name(ref me) = self;
                let &$name(ref you) = other;
                let mut i = $n_words;
                while i > 0 {
                    i -= 1;
                    if me[i] < you[i] { return ::core::cmp::Ordering::Less; }
                    if me[i] > you[i] { return ::core::cmp::Ordering::Greater; }
                }
                ::core::cmp::Ordering::Equal
            }
        }

        impl PartialOrd for $name {
            fn partial_cmp(&self, other: &$name) -> Option<::core::cmp::Ordering> {
                Some(self.cmp(other))
            }
        }

        impl_std_for_uint!($name, $n_words);
        impl_heapsize_for_uint!($name);
    );
}

#[cfg(feature = "std")]
#[macro_export]
#[doc(hidden)]
macro_rules! impl_std_for_uint_internals {
    ($name: ident, $n_words: tt) => {
        /// Convert to hex string.
        #[inline]
        pub fn to_hex(&self) -> String {
            use core::cmp;
            use $crate::rustc_hex::ToHex;;

            if self.is_zero() { return "0".to_owned(); }    // special case.
            let mut bytes = [0u8; 8 * $n_words];
            self.to_big_endian(&mut bytes);
            let bp7 = self.bits() + 7;
            let len = cmp::max(bp7 / 8, 1);
            let bytes_hex = bytes[bytes.len() - len..].to_hex();
            (&bytes_hex[1 - bp7 % 8 / 4..]).to_owned()
        }
    }
}

#[cfg(feature = "std")]
#[macro_export]
#[doc(hidden)]
macro_rules! impl_std_for_uint {
    ($name: ident, $n_words: tt) => {
        impl ::core::fmt::Debug for $name {
            fn fmt(&self, f: &mut ::core::fmt::Formatter) -> ::core::fmt::Result {
                ::core::fmt::Display::fmt(self, f)
            }
        }

        impl ::core::fmt::Display for $name {
            fn fmt(&self, f: &mut ::core::fmt::Formatter) -> ::core::fmt::Result {
                if self.is_zero() {
                    return write!(f, "0");
                }

                let mut s = String::new();
                let mut current = *self;
                let ten = $name::from(10);

                while !current.is_zero() {
                    s = format!("{}{}", (current % ten).low_u32(), s);
                    current = current / ten;
                }

                write!(f, "{}", s)
            }
        }

        impl ::core::str::FromStr for $name {
            type Err = $crate::rustc_hex::FromHexError;

            fn from_str(value: &str) -> Result<$name, Self::Err> {
                use $crate::rustc_hex::FromHex;

                let bytes: Vec<u8> = match value.len() % 2 == 0 {
                    true => try!(value.from_hex()),
                    false => try!(("0".to_owned() + value).from_hex())
                };

                let bytes_ref: &[u8] = &bytes;
                Ok(From::from(bytes_ref))
            }
        }

        impl ::core::fmt::LowerHex for $name {
            fn fmt(&self, f: &mut ::core::fmt::Formatter) -> ::core::fmt::Result {
                let &$name(ref data) = self;
                try!(write!(f, "0x"));
                let mut latch = false;
                for ch in data.iter().rev() {
                    for x in 0..16 {
                        let nibble = (ch & (15u64 << ((15 - x) * 4) as u64)) >>
                                        (((15 - x) * 4) as u64);

                        if !latch { latch = nibble != 0 }
                        if latch {
                            try!(write!(f, "{:x}", nibble));
                        }
                    }
                }
                Ok(())
            }
        }

        impl From<&'static str> for $name {
            fn from(s: &'static str) -> Self {
                s.parse().unwrap()
            }
        }
    }
}

#[cfg(not(feature = "std"))]
#[macro_export]
#[doc(hidden)]
macro_rules! impl_std_for_uint_internals {
    ($name: ident, $n_words: tt) => {}
}

#[cfg(not(feature = "std"))]
#[macro_export]
#[doc(hidden)]
macro_rules! impl_std_for_uint {
    ($name: ident, $n_words: tt) => {}
}

#[cfg(feature = "heapsizeof")]
#[macro_export]
#[doc(hidden)]
macro_rules! impl_heapsize_for_uint {
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
macro_rules! impl_heapsize_for_uint {
    ($name: ident) => {}
}
