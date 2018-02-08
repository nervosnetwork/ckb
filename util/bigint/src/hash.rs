use U256;

#[cfg(feature = "serialize")]
use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[cfg(feature = "serialize")]
use bigint_serialize;

construct_hash!(H32, 4);
construct_hash!(H64, 8);
construct_hash!(H128, 16);
construct_hash!(H160, 20);
construct_hash!(H256, 32);
construct_hash!(H264, 33);
construct_hash!(H512, 64);
construct_hash!(H520, 65);
construct_hash!(H1024, 128);

impl From<U256> for H256 {
    fn from(value: U256) -> H256 {
        let mut ret = H256::new();
        value.to_big_endian(&mut ret);
        ret
    }
}

impl<'a> From<&'a U256> for H256 {
    fn from(value: &'a U256) -> H256 {
        let mut ret: H256 = H256::new();
        value.to_big_endian(&mut ret);
        ret
    }
}

impl From<H256> for U256 {
    fn from(value: H256) -> U256 {
        U256::from(&value)
    }
}

impl<'a> From<&'a H256> for U256 {
    fn from(value: &'a H256) -> U256 {
        U256::from(value.as_ref() as &[u8])
    }
}

impl From<H256> for H160 {
    fn from(value: H256) -> H160 {
        let mut ret = H160::new();
        ret.0.copy_from_slice(&value[12..32]);
        ret
    }
}

impl From<H256> for H64 {
    fn from(value: H256) -> H64 {
        let mut ret = H64::new();
        ret.0.copy_from_slice(&value[20..28]);
        ret
    }
}

impl From<H160> for H256 {
    fn from(value: H160) -> H256 {
        let mut ret = H256::new();
        ret.0[12..32].copy_from_slice(&value);
        ret
    }
}

impl<'a> From<&'a H160> for H256 {
    fn from(value: &'a H160) -> H256 {
        let mut ret = H256::new();
        ret.0[12..32].copy_from_slice(value);
        ret
    }
}

macro_rules! impl_serde {
    ($name: ident, $len: expr) => {
        #[cfg(feature="serialize")]
        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer
            {
                let mut slice = [0u8; 2 + 2 * $len];
                bigint_serialize::serialize(&mut slice, &self.0, serializer)
            }
        }

        #[cfg(feature="serialize")]
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>
            {
                let mut bytes = [0u8; $len];
                bigint_serialize::deserialize_check_len(
                    deserializer,
                    bigint_serialize::ExpectedLen::Exact(&mut bytes)
                )?;
                Ok($name(bytes))
            }
        }
    }
}

impl_serde!(H32, 4);
impl_serde!(H64, 8);
impl_serde!(H128, 16);
impl_serde!(H160, 20);
impl_serde!(H256, 32);
impl_serde!(H264, 33);
impl_serde!(H512, 64);
impl_serde!(H520, 65);
impl_serde!(H1024, 128);
