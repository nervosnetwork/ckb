//! Map a monotonic target window to Unix time without timing wall-clock reads.
//!
//! The independent endpoint anchor detects wall corrections and slow clock
//! reads for profile attribution. Neither changes the measured target duration.

use std::time::{Instant, SystemTime, UNIX_EPOCH};

pub(crate) struct ClockAnchor {
    midpoint: Instant,
    unix_nanos: u128,
    uncertainty_nanos: u128,
}

impl ClockAnchor {
    pub(crate) fn capture() -> Result<Self, String> {
        let before = Instant::now();
        let wall = SystemTime::now();
        let after = Instant::now();
        Self::from_observations(
            before,
            wall.duration_since(UNIX_EPOCH)
                .map_err(|error| error.to_string())?
                .as_nanos(),
            after,
        )
    }

    fn from_observations(
        before: Instant,
        unix_nanos: u128,
        after: Instant,
    ) -> Result<Self, String> {
        let bracket = after
            .checked_duration_since(before)
            .ok_or("anchor monotonic clock moved backwards")?;
        Ok(Self {
            midpoint: before + bracket / 2,
            unix_nanos,
            uncertainty_nanos: bracket.as_nanos().div_ceil(2),
        })
    }

    pub(crate) fn map(&self, instant: Instant) -> Result<u128, String> {
        if let Some(delta) = instant.checked_duration_since(self.midpoint) {
            self.unix_nanos.checked_add(delta.as_nanos())
        } else {
            self.unix_nanos
                .checked_sub(self.midpoint.duration_since(instant).as_nanos())
        }
        .ok_or_else(|| "measurement Unix timestamp overflow".into())
    }

    pub(crate) fn window(
        &self,
        scenario: &str,
        started: Instant,
        ended: Instant,
        endpoint: &Self,
    ) -> Result<serde_json::Value, String> {
        let elapsed = ended
            .checked_duration_since(started)
            .filter(|duration| !duration.is_zero())
            .ok_or("measurement monotonic window is empty or reversed")?
            .as_nanos();
        let start = self.map(started)?;
        let end = start
            .checked_add(elapsed)
            .ok_or("measurement Unix timestamp overflow")?;
        Ok(serde_json::json!({
            "schema_version": 3,
            "scenario": scenario,
            "start_unix_nanos": start,
            "end_unix_nanos": end,
            "elapsed_nanos": elapsed,
            "start_clock_uncertainty_nanos": self.uncertainty_nanos,
            "end_clock_uncertainty_nanos": endpoint.uncertainty_nanos,
            "observed_end_unix_nanos": endpoint.map(ended)?,
        }))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn target_excludes_preempted_anchor_reads_and_adjacent_accounting() {
        use super::*;
        use std::time::Duration;
        let origin = Instant::now();
        let at = |nanos| origin + Duration::from_nanos(nanos);
        // 20ns clock-read brackets, 50ns/80ns accounting gaps outside target.
        let start = ClockAnchor::from_observations(at(0), 1_000, at(20)).unwrap();
        let end = ClockAnchor::from_observations(at(170), 1_170, at(190)).unwrap();
        let window = start.window("fixture", at(70), at(90), &end).unwrap();
        assert_eq!(window["start_unix_nanos"], 1_060);
        assert_eq!(window["end_unix_nanos"], 1_080);
        assert_eq!(window["observed_end_unix_nanos"], 1_080);
        assert_eq!(window["elapsed_nanos"], 20);
        assert_eq!(window["start_clock_uncertainty_nanos"], 10);
        assert_eq!(window["end_clock_uncertainty_nanos"], 10);
    }

    #[test]
    fn wall_adjustment_only_moves_independent_profile_alignment() {
        use super::*;
        use std::time::Duration;
        let origin = Instant::now();
        let at = |nanos| origin + Duration::from_nanos(nanos);
        let start = ClockAnchor::from_observations(at(0), 1_000, at(0)).unwrap();
        for wall in [1_200, 1_000] {
            let end = ClockAnchor::from_observations(at(100), wall, at(100)).unwrap();
            let window = start.window("fixture", at(10), at(90), &end).unwrap();
            assert_eq!(window["elapsed_nanos"], 80);
            assert_eq!(window["end_unix_nanos"], 1_090);
            assert_eq!(window["observed_end_unix_nanos"], (wall - 10) as u64);
        }
    }

    #[test]
    fn invalid_clocks_and_timestamp_overflow_fail_closed() {
        use super::*;
        use std::time::Duration;
        let now = Instant::now();
        let later = now + Duration::from_nanos(1);
        assert!(ClockAnchor::from_observations(later, 0, now).is_err());
        let anchor = ClockAnchor::from_observations(now, 0, now).unwrap();
        assert!(anchor.window("fixture", now, now, &anchor).is_err());
        assert!(anchor.window("fixture", later, now, &anchor).is_err());
        assert!(anchor.map(now - Duration::from_nanos(1)).is_err());
        let overflow = ClockAnchor::from_observations(now, u128::MAX, now).unwrap();
        assert!(overflow.window("fixture", now, later, &overflow).is_err());
        assert!(ClockAnchor::capture().is_ok());
    }
}
