use super::*;

pub(in crate::authority) fn replacement_changes_accepted_source_for_foundation(
    before: &OwnedTx,
    after: &OwnedTx,
) -> bool {
    matches!(
        SourceImpact::for_replacement(Some(before), Some(after)),
        SourceImpact::Accepted
    )
}
