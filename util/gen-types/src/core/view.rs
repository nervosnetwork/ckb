#[cfg(not(feature = "std"))]
use crate::util::hash::new_blake2b;
#[cfg(feature = "std")]
use ckb_hash::new_blake2b;

use crate::generated::packed;
use crate::prelude::*;

/// A readonly and immutable struct which includes extra hash and the decoupled
/// parts of it.
#[derive(Debug, Clone)]
pub struct ExtraHashView {
    /// The uncles hash which is used to combine to the extra hash.
    pub(crate) uncles_hash: packed::Byte32,
    /// The first item is the new field hash, which is used to combine to the extra hash.
    /// The second item is the extra hash.
    pub(crate) extension_hash_and_extra_hash: Option<(packed::Byte32, packed::Byte32)>,
}

impl ExtraHashView {
    /// Creates `ExtraHashView` with `uncles_hash` and optional `extension_hash`.
    pub fn new(uncles_hash: packed::Byte32, extension_hash_opt: Option<packed::Byte32>) -> Self {
        let extension_hash_and_extra_hash = extension_hash_opt.map(|extension_hash| {
            let mut ret = [0u8; 32];
            let mut blake2b = new_blake2b();
            blake2b.update(uncles_hash.as_slice());
            blake2b.update(extension_hash.as_slice());
            blake2b.finalize(&mut ret);
            (extension_hash, ret.pack())
        });
        Self {
            uncles_hash,
            extension_hash_and_extra_hash,
        }
    }

    /// Gets `uncles_hash`.
    pub fn uncles_hash(&self) -> packed::Byte32 {
        self.uncles_hash.clone()
    }

    /// Gets `extension_hash`.
    pub fn extension_hash(&self) -> Option<packed::Byte32> {
        self.extension_hash_and_extra_hash
            .as_ref()
            .map(|(ref extension_hash, _)| extension_hash.clone())
    }

    /// Gets `extra_hash`.
    pub fn extra_hash(&self) -> packed::Byte32 {
        self.extension_hash_and_extra_hash
            .as_ref()
            .map(|(_, ref extra_hash)| extra_hash.clone())
            .unwrap_or_else(|| self.uncles_hash.clone())
    }
}
