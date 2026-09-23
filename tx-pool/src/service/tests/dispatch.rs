use super::*;

#[tokio::test]
async fn public_operational_failure_returns_error_without_faulting_the_service() {
    for error in [
        Error::Stale,
        Error::Full("fixture pressure".into()),
        Error::Closed,
    ] {
        let (sender, receiver) = tokio::sync::oneshot::channel::<Result<(), AnyError>>();
        assert!(reply_result(sender, Err(error.clone()), "fixture").is_ok());
        let returned = receiver.await.unwrap().unwrap_err();
        assert_eq!(
            returned.downcast_ref::<Error>().unwrap().to_string(),
            error.to_string()
        );
    }
    let (sender, receiver) = tokio::sync::oneshot::channel::<Result<(), AnyError>>();
    assert!(matches!(
        reply_result(sender, Err(Error::Fault("fixture integrity")), "fixture"),
        Err(Error::Fault(_))
    ));
    assert!(matches!(
        receiver.await.unwrap().unwrap_err().downcast_ref::<Error>(),
        Some(Error::Fault(_))
    ));
}

#[tokio::test]
async fn local_competing_progress_keeps_the_public_typed_error_discriminator() {
    let (sender, receiver) = tokio::sync::oneshot::channel::<Result<(), AnyError>>();
    assert!(
        reply_external(
            sender,
            Err(crate::service::LocalRemovalCompetingProgress.into()),
            "remove_local_tx"
        )
        .is_ok()
    );
    assert!(
        receiver
            .await
            .unwrap()
            .unwrap_err()
            .downcast_ref::<crate::service::LocalRemovalCompetingProgress>()
            .is_some()
    );
}

#[test]
fn remote_batch_result_reports_only_its_exact_committed_prefix() {
    let outcome = RemoteTxBatchOutcome::failed(3, 2, Error::Closed.into());
    let (offered, completed, error) = outcome.into_parts();
    assert_eq!((offered, completed), (3, 2));
    assert!(matches!(
        error.unwrap().downcast_ref::<Error>(),
        Some(Error::Closed)
    ));
    let outcome = RemoteTxBatchOutcome::complete(3);
    assert_eq!((outcome.offered(), outcome.completed()), (3, 3));
}
