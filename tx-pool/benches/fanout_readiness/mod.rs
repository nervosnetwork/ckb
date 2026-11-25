//! Observable readiness barriers for the bounded reverse-fanout workload.

use std::time::{Duration, Instant};

pub(crate) const CHILDREN: usize = 64;

#[derive(Default)]
pub(crate) struct Readiness {
    pub(crate) queries: u64,
    pub(crate) barriers: u64,
    pub(crate) elapsed_nanos: u128,
}

impl Readiness {
    pub(crate) async fn wait(
        &mut self,
        expected: usize,
        mut query: impl FnMut() -> Result<usize, String>,
    ) -> Result<(), String> {
        self.wait_until(
            expected,
            Instant::now() + Duration::from_secs(30),
            &mut query,
        )
        .await
    }

    async fn wait_until(
        &mut self,
        expected: usize,
        deadline: Instant,
        query: &mut impl FnMut() -> Result<usize, String>,
    ) -> Result<(), String> {
        let started = Instant::now();
        let result = async {
            loop {
                if Instant::now() >= deadline {
                    return Err(format!("orphan readiness deadline: expected={expected}"));
                }
                self.queries += 1;
                let observed = query()?;
                if observed > CHILDREN {
                    return Err(format!("orphan readiness exceeded cohort: {observed}"));
                }
                if observed == expected {
                    self.barriers += 1;
                    return Ok(());
                }
                // No public readiness notification exists in both versions.
                // Yield between queries; the query/wait cost remains timed.
                tokio::task::yield_now().await;
            }
        }
        .await;
        self.elapsed_nanos += started.elapsed().as_nanos();
        result
    }
}

#[cfg(test)]
mod tests {

    #[tokio::test]
    async fn only_exact_observed_counts_open_each_barrier() {
        use super::*;
        let mut readiness = Readiness::default();
        let mut draining = [64, 1, 0].into_iter();
        readiness
            .wait(0, || Ok(draining.next().unwrap()))
            .await
            .unwrap();
        let mut filling = [0, 1, 63, 64].into_iter();
        readiness
            .wait(64, || Ok(filling.next().unwrap()))
            .await
            .unwrap();
        assert_eq!(readiness.queries, 7);
        assert_eq!(readiness.barriers, 2);
    }

    #[tokio::test]
    async fn query_failure_and_foreign_population_never_open_barrier() {
        use super::*;
        let mut readiness = Readiness::default();
        assert!(
            readiness
                .wait(64, || Err("service stopped".into()))
                .await
                .is_err()
        );
        assert!(readiness.wait(64, || Ok(65)).await.is_err());
        assert_eq!(readiness.barriers, 0);
    }

    #[tokio::test]
    async fn expired_deadline_never_queries_or_opens_barrier() {
        use super::*;
        let mut readiness = Readiness::default();
        assert!(
            readiness
                .wait_until(64, Instant::now(), &mut || {
                    panic!("expired barrier queried service")
                })
                .await
                .is_err()
        );
        assert_eq!(readiness.queries, 0);
        assert_eq!(readiness.barriers, 0);
    }
}
