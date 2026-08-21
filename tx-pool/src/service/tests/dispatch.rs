use super::*;

#[test]
fn remote_partial_commit_acknowledges_only_the_committed_prefix() {
    let (first_tx, first_rx) = tokio::sync::oneshot::channel();
    let (second_tx, second_rx) = tokio::sync::oneshot::channel();
    let (suffix_tx, suffix_rx) = tokio::sync::oneshot::channel();

    assert!(
        settle_remote_responder_prefix(
            vec![first_tx, second_tx, suffix_tx],
            2,
            Some(AuthorityServiceError::Cancelled),
        )
        .is_ok(),
        "cancellation is operational after the exact committed prefix is acknowledged",
    );

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("the response probe runtime builds");
    assert_eq!(runtime.block_on(first_rx), Ok(()));
    assert_eq!(runtime.block_on(second_rx), Ok(()));
    assert!(
        runtime.block_on(suffix_rx).is_err(),
        "the uncommitted suffix receives a closed-channel negative acknowledgement",
    );
}

#[test]
fn impossible_remote_progress_invalidates_the_generation_without_acknowledging_past_the_batch() {
    let (responder, response) = tokio::sync::oneshot::channel();
    let progress =
        crate::authority::service::RemoteIngressBatchProgress::complete_for_foundation(2);
    let (completed, error) = progress.into_checked_parts(1);
    assert!(
        settle_remote_responder_prefix(vec![responder], completed, error).is_err(),
        "completed progress outside the responder capability is structural",
    );

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("the response probe runtime builds");
    assert_eq!(runtime.block_on(response), Ok(()));
}
