use crate::{H160, H256, H512, H520};

macro_rules! impl_std_hash_hash {
    ($name:ident, $bytes_size:expr) => {
        impl ::std::hash::Hash for $name {
            #[inline]
            fn hash<H: ::std::hash::Hasher>(&self, state: &mut H) {
                state.write(&self.0[..])
            }
        }
    };
}

impl_std_hash_hash!(H160, 20);
impl_std_hash_hash!(H256, 32);
impl_std_hash_hash!(H512, 64);
impl_std_hash_hash!(H520, 65);
