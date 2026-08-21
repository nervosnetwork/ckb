use super::*;

impl ChainReorgPayloadLimit {
    pub(crate) const fn for_test(bytes: usize) -> Self {
        Self(bytes)
    }
}

impl ChainReorgArgs {
    pub(crate) fn is_detailed(&self) -> bool {
        matches!(self, Self::Detailed { .. })
    }
}
