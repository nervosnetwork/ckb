use ckb_network::PeerIndex;
use governor::{
    Quota, RateLimiter,
    clock::{Clock, DefaultClock},
    middleware::NoOpMiddleware,
    state::keyed::HashMapStateStore,
};
use std::num::NonZeroU32;

// Allow ten maximum-size batches per second, with twenty batches of burst
// capacity to accommodate header-sync pipelining and network jitter.
const HEADERS_PER_SECOND: u32 = 20_000;
const HEADERS_BURST: u32 = 40_000;

pub(super) struct HeadersRateLimiter<C: Clock = DefaultClock> {
    limiter: RateLimiter<PeerIndex, HashMapStateStore<PeerIndex>, C, NoOpMiddleware<C::Instant>>,
}

impl Default for HeadersRateLimiter {
    fn default() -> Self {
        Self::new(DefaultClock::default())
    }
}

impl<C: Clock> HeadersRateLimiter<C> {
    fn new(clock: C) -> Self {
        let quota = Quota::per_second(NonZeroU32::new(HEADERS_PER_SECOND).unwrap())
            .allow_burst(NonZeroU32::new(HEADERS_BURST).unwrap());
        Self {
            limiter: RateLimiter::new(quota, HashMapStateStore::default(), clock),
        }
    }

    pub(super) fn check(&self, peer: PeerIndex, headers: usize) -> bool {
        // Empty responses still consume parsing and sync-state work. Do not let
        // integer conversion truncate the cost if a caller omits the batch cap.
        let Ok(cost) = u32::try_from(headers.max(1)) else {
            return false;
        };
        matches!(
            self.limiter
                .check_key_n(&peer, NonZeroU32::new(cost).unwrap()),
            Ok(Ok(()))
        )
    }

    pub(super) fn retain_recent(&self) {
        self.limiter.retain_recent();
        self.limiter.shrink_to_fit();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ckb_constant::sync::MAX_HEADERS_LEN;
    use governor::clock::FakeRelativeClock;
    use std::time::Duration;

    #[test]
    fn budget_is_weighted_atomic_and_independent_per_peer() {
        let limiter = HeadersRateLimiter::new(FakeRelativeClock::default());
        assert!(limiter.check(1.into(), HEADERS_BURST as usize - MAX_HEADERS_LEN));
        assert!(limiter.check(1.into(), MAX_HEADERS_LEN - 1));
        assert!(!limiter.check(1.into(), MAX_HEADERS_LEN));
        // Rejection must not consume the remaining single-header allowance.
        assert!(limiter.check(1.into(), 1));
        assert!(!limiter.check(1.into(), 1));
        assert!(limiter.check(2.into(), MAX_HEADERS_LEN));
    }

    #[test]
    fn empty_responses_cost_one_and_budget_refills() {
        let clock = FakeRelativeClock::default();
        let limiter = HeadersRateLimiter::new(clock.clone());
        assert!(limiter.check(1.into(), HEADERS_BURST as usize - 1));
        assert!(limiter.check(1.into(), 0));
        assert!(!limiter.check(1.into(), 0));
        clock.advance(Duration::from_secs(1));
        assert!(limiter.check(1.into(), HEADERS_PER_SECOND as usize));
        assert!(!limiter.check(1.into(), 1));
        // Cleanup expires records strictly after their refill deadline.
        clock.advance(Duration::from_secs(3));
        limiter.retain_recent();
        assert!(limiter.limiter.is_empty());
        assert!(limiter.check(1.into(), HEADERS_BURST as usize));
    }

    #[test]
    fn maximum_batches_fit_but_oversized_costs_do_not_wrap() {
        let limiter = HeadersRateLimiter::new(FakeRelativeClock::default());
        assert!(!limiter.check(1.into(), usize::MAX));
        for _ in 0..HEADERS_BURST as usize / MAX_HEADERS_LEN {
            assert!(limiter.check(1.into(), MAX_HEADERS_LEN));
        }
        assert!(!limiter.check(1.into(), MAX_HEADERS_LEN));
    }

    #[test]
    fn exhausted_budget_rejects_before_header_processing_and_preserves_batch_cap() {
        use crate::relayer::tests::helper::{MockProtocolContext, build_chain};
        use crate::synchronizer::HeadersProcess;
        use crate::{Status, StatusCode, Synchronizer};
        use ckb_network::{CKBProtocolContext, SupportProtocols};
        use ckb_types::{packed, prelude::*};
        use std::sync::Arc;

        let (chain, relayer, _) = build_chain(2);
        let shared = Arc::clone(relayer.shared());
        let mut sync = Synchronizer::new(chain.chain_controller().clone(), Arc::clone(&shared));
        // Give this test a slow refill instead of relying on wall-clock sleeps.
        sync.headers_rate_limiter = HeadersRateLimiter {
            limiter: RateLimiter::hashmap(
                Quota::per_hour(NonZeroU32::new(1).unwrap())
                    .allow_burst(NonZeroU32::new(2).unwrap()),
            ),
        };
        assert!(sync.headers_rate_limiter.check(1.into(), 2));
        let mock = Arc::new(MockProtocolContext::new(SupportProtocols::Sync));
        let nc: Arc<dyn CKBProtocolContext + Sync> = Arc::<MockProtocolContext>::clone(&mock);
        let unverified = packed::Header::default();
        let hash = unverified.calc_header_hash();
        let message = packed::SendHeaders::new_builder()
            .headers(vec![unverified.clone(), unverified.clone()])
            .build();
        let status = HeadersProcess::new(message.as_reader(), &sync, 1.into(), &nc).execute();
        assert_eq!(status.code(), StatusCode::TooManyRequests);
        assert!(status.should_ban().is_none());
        assert_eq!(mock.disconnected_peers(), vec![1.into()]);
        assert!(shared.shared().header_map().get(&hash).is_none());
        assert!(shared.shared().get_block_status(&hash).is_empty());
        assert_eq!(mock.sent_messages_len(), 0);

        let oversized = packed::SendHeaders::new_builder()
            .headers(vec![unverified; MAX_HEADERS_LEN + 1])
            .build();
        let status = HeadersProcess::new(oversized.as_reader(), &sync, 1.into(), &nc).execute();
        assert_eq!(status.code(), StatusCode::HeadersIsInvalid);
        assert_eq!(
            status.should_ban(),
            Some(ckb_constant::sync::BAD_MESSAGE_BAN_TIME)
        );

        // Another peer can still finish a normal header exchange, including an
        // empty response and a replay of a known valid header.
        let empty = packed::SendHeaders::default();
        assert_eq!(
            HeadersProcess::new(empty.as_reader(), &sync, 2.into(), &nc).execute(),
            Status::ok()
        );
        let known = packed::SendHeaders::new_builder()
            .headers(vec![shared.active_chain().tip_header().data()])
            .build();
        assert_eq!(
            HeadersProcess::new(known.as_reader(), &sync, 2.into(), &nc).execute(),
            Status::ok()
        );
        assert_eq!(mock.disconnected_peers(), vec![1.into()]);
    }
}
