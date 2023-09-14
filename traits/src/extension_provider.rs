use ckb_types::packed;

/// Trait for block extension field storage
pub trait ExtensionProvider {
    /// Get the extension field of the given block hash
    fn get_block_extension(&self, hash: &packed::Byte32) -> Option<packed::Bytes>;
}
