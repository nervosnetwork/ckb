use super::*;

impl DirectTransactionRejection {
    pub(in crate::authority) fn transaction(&self) -> &Arc<TransactionView> {
        &self.tx
    }

    pub(in crate::authority) fn physical_read_support_for_foundation(
        &self,
        authority: &super::super::plan::TxPoolAuthority,
    ) -> super::super::shard::ShardReadSupport {
        match &self.validity {
            DirectRejectionValidity::Stable => Default::default(),
            DirectRejectionValidity::AcceptedReads { reads, .. } => {
                reads.sharded_read_support(authority.entries_for_reference())
            }
        }
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
