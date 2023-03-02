// use crate::core::EpochNumber;
// use paste::paste;

/// A switch to select hard fork features base on the epoch number.
///
/// For safety, all fields are private and not allowed to update.
/// This structure can only be constructed by [`CKB2023Builder`].
///
/// [`CKB2023Builder`]:  struct.CKB2023Builder.html
#[derive(Debug, Clone)]
pub struct CKB2023 {}

/// Builder for [`CKB2023`].
///
/// [`CKB2023`]:  struct.CKB2023.html
#[derive(Debug, Clone, Default)]
pub struct CKB2023Builder {}

impl CKB2023 {
    /// Creates a new builder to build an instance.
    pub fn new_builder() -> CKB2023Builder {
        Default::default()
    }

    /// Creates a new builder based on the current instance.
    pub fn as_builder(&self) -> CKB2023Builder {
        Self::new_builder()
    }
}

impl CKB2023Builder {
    /// Build a new [`CKB2023`].
    ///
    /// Returns an error if failed at any check, for example, there maybe are some features depend
    /// on others.
    ///
    /// [`CKB2023`]: struct.CKB2023.html
    pub fn build(self) -> Result<CKB2023, String> {
        Ok(CKB2023 {})
    }
}
