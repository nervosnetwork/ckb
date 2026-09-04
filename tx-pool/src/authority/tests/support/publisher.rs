use super::*;

#[derive(Debug)]
pub(in crate::authority) enum FoundationPublisherError {
    ConcurrentConsumer,
    Publisher(
        #[expect(
            dead_code,
            reason = "the fixture preserves the exact publisher fault for expect/panic diagnostics"
        )]
        AuthorityEffectPublisherFault,
    ),
}

impl AuthorityEffectEndpoints {
    pub(in crate::authority) fn publish(&mut self, mut outcome: CompiledEndpointOutcome) {
        for endpoint in EffectEndpoint::ORDER {
            self.publish_endpoint(&mut outcome, endpoint);
        }
    }
}

impl BanAction {
    pub(in crate::authority) const fn peer(&self) -> ckb_network::PeerIndex {
        self.lease.peer()
    }

    pub(in crate::authority) fn remaining_duration_at(&self, now: Instant) -> Option<Duration> {
        self.lease.remaining_at(now)
    }
}

/// Test harness for acquiring the same move-only publication claim that
/// production topology acquires synchronously before spawning the publisher.
pub(in crate::authority) async fn run_authority_effect_publisher(
    runtime: AuthorityRuntime,
    endpoints: AuthorityEffectEndpoints,
) -> Result<(), FoundationPublisherError> {
    let Some(claim) = runtime.claim_effect_publisher() else {
        return Err(FoundationPublisherError::ConcurrentConsumer);
    };
    run_claimed_authority_effect_publisher(runtime, endpoints, claim)
        .await
        .map_err(FoundationPublisherError::Publisher)
}
