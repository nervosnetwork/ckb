use super::*;

pub(in crate::authority) fn replacement_changes_both_template_sources_for_foundation(
    before: &OwnedTx,
    after: &OwnedTx,
) -> bool {
    matches!(
        SourceImpact::for_replacement(Some(before), Some(after)),
        SourceImpact::Accepted
    )
}
