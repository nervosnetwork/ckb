//! Role-separated dependency identities used by evidence properties.

/// Cells and headers with the same finite-domain ordinal remain distinct
/// because production stores them in distinct `DependencyKey` variants and
/// their legal missing outcomes differ.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum ClaimDependencyKey {
    Cell(u8),
    Header(u8),
}

impl ClaimDependencyKey {
    pub(crate) const fn cell(value: u8) -> Self {
        Self::Cell(value)
    }

    pub(crate) const fn header(value: u8) -> Self {
        Self::Header(value)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct ClaimDependencyCut(pub(crate) u16);
