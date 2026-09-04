use super::*;

impl DirectTransactionRejection {
    pub(in crate::authority) fn transaction(&self) -> &Arc<TransactionView> {
        &self.tx
    }
}

impl From<super::super::state::test_support::RejectionKind> for CommittedPublicReject {
    fn from(reason: super::super::state::test_support::RejectionKind) -> Self {
        let reject = match reason {
            super::super::state::test_support::RejectionKind::Verification => {
                Reject::Invalidated("foundation verification rejection".to_owned())
            }
            super::super::state::test_support::RejectionKind::Policy => {
                Reject::Invalidated("foundation policy rejection".to_owned())
            }
        };
        Self::new(reject)
    }
}
