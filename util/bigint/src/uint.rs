#[cfg(feature = "serialize")]
use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[cfg(feature = "serialize")]
use bigint_serialize;

construct_uint!(U128, 2);
construct_uint!(U256, 4);
construct_uint!(U512, 8);

impl U256 {
    /// Multiplies two 256-bit integers to produce full 512-bit integer
    /// No overflow possible
    #[cfg(all(asm_available, target_arch = "x86_64"))]
    pub fn full_mul(self, other: U256) -> U512 {
        let self_t: &[u64; 4] = &self.0;
        let other_t: &[u64; 4] = &other.0;
        let mut result: [u64; 8] = unsafe { ::core::mem::uninitialized() };
        unsafe {
            asm!("
                mov $8, %rax
                mulq $12
                mov %rax, $0
                mov %rdx, $1

                mov $8, %rax
                mulq $13
                add %rax, $1
                adc $$0, %rdx
                mov %rdx, $2

                mov $8, %rax
                mulq $14
                add %rax, $2
                adc $$0, %rdx
                mov %rdx, $3

                mov $8, %rax
                mulq $15
                add %rax, $3
                adc $$0, %rdx
                mov %rdx, $4

                mov $9, %rax
                mulq $12
                add %rax, $1
                adc %rdx, $2
                adc $$0, $3
                adc $$0, $4
                xor $5, $5
                adc $$0, $5
                xor $6, $6
                adc $$0, $6
                xor $7, $7
                adc $$0, $7

                mov $9, %rax
                mulq $13
                add %rax, $2
                adc %rdx, $3
                adc $$0, $4
                adc $$0, $5
                adc $$0, $6
                adc $$0, $7

                mov $9, %rax
                mulq $14
                add %rax, $3
                adc %rdx, $4
                adc $$0, $5
                adc $$0, $6
                adc $$0, $7

                mov $9, %rax
                mulq $15
                add %rax, $4
                adc %rdx, $5
                adc $$0, $6
                adc $$0, $7

                mov $10, %rax
                mulq $12
                add %rax, $2
                adc %rdx, $3
                adc $$0, $4
                adc $$0, $5
                adc $$0, $6
                adc $$0, $7

                mov $10, %rax
                mulq $13
                add %rax, $3
                adc %rdx, $4
                adc $$0, $5
                adc $$0, $6
                adc $$0, $7

                mov $10, %rax
                mulq $14
                add %rax, $4
                adc %rdx, $5
                adc $$0, $6
                adc $$0, $7

                mov $10, %rax
                mulq $15
                add %rax, $5
                adc %rdx, $6
                adc $$0, $7

                mov $11, %rax
                mulq $12
                add %rax, $3
                adc %rdx, $4
                adc $$0, $5
                adc $$0, $6
                adc $$0, $7

                mov $11, %rax
                mulq $13
                add %rax, $4
                adc %rdx, $5
                adc $$0, $6
                adc $$0, $7

                mov $11, %rax
                mulq $14
                add %rax, $5
                adc %rdx, $6
                adc $$0, $7

                mov $11, %rax
                mulq $15
                add %rax, $6
                adc %rdx, $7
                "
            : /* $0 */ "={r8}"(result[0]), /* $1 */ "={r9}"(result[1]), /* $2 */ "={r10}"(result[2]),
              /* $3 */ "={r11}"(result[3]), /* $4 */ "={r12}"(result[4]), /* $5 */ "={r13}"(result[5]),
              /* $6 */ "={r14}"(result[6]), /* $7 */ "={r15}"(result[7])

            : /* $8 */ "m"(self_t[0]), /* $9 */ "m"(self_t[1]), /* $10 */  "m"(self_t[2]),
              /* $11 */ "m"(self_t[3]), /* $12 */ "m"(other_t[0]), /* $13 */ "m"(other_t[1]),
              /* $14 */ "m"(other_t[2]), /* $15 */ "m"(other_t[3])
            : "rax", "rdx"
            :
            );
        }
        U512(result)
    }

    /// Multiplies two 256-bit integers to produce full 512-bit integer
    /// No overflow possible
    #[inline(always)]
    #[cfg(not(all(asm_available, target_arch = "x86_64")))]
    pub fn full_mul(self, other: U256) -> U512 {
        U512(uint_full_mul_reg!(U256, 4, self, other))
    }
}

impl From<U256> for U512 {
    fn from(value: U256) -> U512 {
        let U256(ref arr) = value;
        let mut ret = [0; 8];
        ret[0] = arr[0];
        ret[1] = arr[1];
        ret[2] = arr[2];
        ret[3] = arr[3];
        U512(ret)
    }
}

impl From<U512> for U256 {
    fn from(value: U512) -> U256 {
        let U512(ref arr) = value;
        if arr[4] | arr[5] | arr[6] | arr[7] != 0 {
            panic!("Overflow");
        }
        let mut ret = [0; 4];
        ret[0] = arr[0];
        ret[1] = arr[1];
        ret[2] = arr[2];
        ret[3] = arr[3];
        U256(ret)
    }
}

impl<'a> From<&'a U256> for U512 {
    fn from(value: &'a U256) -> U512 {
        let U256(ref arr) = *value;
        let mut ret = [0; 8];
        ret[0] = arr[0];
        ret[1] = arr[1];
        ret[2] = arr[2];
        ret[3] = arr[3];
        U512(ret)
    }
}

impl<'a> From<&'a U512> for U256 {
    fn from(value: &'a U512) -> U256 {
        let U512(ref arr) = *value;
        if arr[4] | arr[5] | arr[6] | arr[7] != 0 {
            panic!("Overflow");
        }
        let mut ret = [0; 4];
        ret[0] = arr[0];
        ret[1] = arr[1];
        ret[2] = arr[2];
        ret[3] = arr[3];
        U256(ret)
    }
}

impl From<U256> for U128 {
    fn from(value: U256) -> U128 {
        let U256(ref arr) = value;
        if arr[2] | arr[3] != 0 {
            panic!("Overflow");
        }
        let mut ret = [0; 2];
        ret[0] = arr[0];
        ret[1] = arr[1];
        U128(ret)
    }
}

impl From<U512> for U128 {
    fn from(value: U512) -> U128 {
        let U512(ref arr) = value;
        if arr[2] | arr[3] | arr[4] | arr[5] | arr[6] | arr[7] != 0 {
            panic!("Overflow");
        }
        let mut ret = [0; 2];
        ret[0] = arr[0];
        ret[1] = arr[1];
        U128(ret)
    }
}

impl From<U128> for U512 {
    fn from(value: U128) -> U512 {
        let U128(ref arr) = value;
        let mut ret = [0; 8];
        ret[0] = arr[0];
        ret[1] = arr[1];
        U512(ret)
    }
}

impl From<U128> for U256 {
    fn from(value: U128) -> U256 {
        let U128(ref arr) = value;
        let mut ret = [0; 4];
        ret[0] = arr[0];
        ret[1] = arr[1];
        U256(ret)
    }
}

impl From<U256> for u64 {
    fn from(value: U256) -> u64 {
        value.as_u64()
    }
}

impl From<U256> for u32 {
    fn from(value: U256) -> u32 {
        value.as_u32()
    }
}

impl<'a> From<&'a [u8; 32]> for U256 {
    fn from(bytes: &[u8; 32]) -> Self {
        bytes[..].into()
    }
}

impl From<[u8; 32]> for U256 {
    fn from(bytes: [u8; 32]) -> Self {
        bytes[..].as_ref().into()
    }
}

impl From<U256> for [u8; 32] {
    fn from(number: U256) -> Self {
        let mut arr = [0u8; 32];
        number.to_big_endian(&mut arr);
        arr
    }
}

impl<'a> From<&'a [u8; 16]> for U128 {
    fn from(bytes: &[u8; 16]) -> Self {
        bytes[..].into()
    }
}

impl From<[u8; 16]> for U128 {
    fn from(bytes: [u8; 16]) -> Self {
        bytes[..].as_ref().into()
    }
}

impl From<U128> for [u8; 16] {
    fn from(number: U128) -> Self {
        let mut arr = [0u8; 16];
        number.to_big_endian(&mut arr);
        arr
    }
}

impl<'a> From<&'a [u8; 64]> for U512 {
    fn from(bytes: &[u8; 64]) -> Self {
        bytes[..].into()
    }
}

impl From<[u8; 64]> for U512 {
    fn from(bytes: [u8; 64]) -> Self {
        bytes[..].as_ref().into()
    }
}

impl From<U512> for [u8; 64] {
    fn from(number: U512) -> Self {
        let mut arr = [0u8; 64];
        number.to_big_endian(&mut arr);
        arr
    }
}

macro_rules! impl_serde {
    ($name: ident, $len: expr) => {
        #[cfg(feature="serialize")]
        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error> where S: Serializer {
                let mut bytes = [0u8; $len * 8];
                self.to_big_endian(&mut bytes);
                bigint_serialize::serialize_uint(&bytes, serializer)
            }
        }

        #[cfg(feature="serialize")]
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error> where D: Deserializer<'de> {
                bigint_serialize::deserialize_check_len(deserializer,
                                                        bigint_serialize::
                                                        ExpectedLen::Between(0, $len * 8))
                    .map(|x| (&*x).into())
            }
        }
    }
}

impl_serde!(U128, 2);
impl_serde!(U256, 4);
impl_serde!(U512, 8);
