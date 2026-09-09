//! Optional entered-scope wall-time counters for the one-shot harness.
//!
//! Span references may outlive their operations (including through Tokio's
//! blocking worker handoff). Integrate active entry multiplicity at each event
//! and at the window cutoff; never wait for those references to close. Nested
//! and concurrent entries overlap, so these durations are not additive CPU.

use std::{
    sync::{Arc, Mutex},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

const NAMES: [&str; 11] = [
    "tx_pool.authority.acquire",
    "tx_pool.authority.apply",
    "tx_pool.authority.capture",
    "tx_pool.chain.reconcile",
    "tx_pool.chain.recover",
    "tx_pool.effects.publish",
    "tx_pool.ingress.remote_batch",
    "tx_pool.maintenance.wake",
    "tx_pool.membership.admission",
    "tx_pool.stage.resolve",
    "tx_pool.stage.verify",
];

#[derive(Clone, Copy, Default)]
struct Counter {
    start_count: u64,
    enter_count: u64,
    active_at_start: u64,
    active: u64,
    updated_nanos: u128,
    elapsed_nanos: u128,
}

impl Counter {
    fn advance(&mut self, now: u128) {
        self.elapsed_nanos += (now - self.updated_nanos) * u128::from(self.active);
        self.updated_nanos = now;
    }
}

#[derive(Default)]
struct State {
    started: Option<Instant>,
    start_unix_nanos: u128,
    spans: [Counter; NAMES.len()],
    unknown: u64,
}

#[derive(Default)]
struct Counters(Mutex<State>);

impl Counters {
    fn begin(&self) -> Result<(), String> {
        let mut state = self.0.lock().expect("profile counter lock poisoned");
        if state.started.is_some() {
            return Err("profile counter window is already active".into());
        }
        for span in &mut state.spans {
            *span = Counter {
                active: span.active,
                active_at_start: span.active,
                ..Counter::default()
            };
        }
        state.unknown = 0;
        state.start_unix_nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| error.to_string())?
            .as_nanos();
        state.started = Some(Instant::now());
        Ok(())
    }

    fn start_span(&self, name: &str) -> Option<usize> {
        let index = NAMES.iter().position(|candidate| *candidate == name);
        let mut state = self.0.lock().expect("profile counter lock poisoned");
        if state.started.is_some() {
            if let Some(index) = index {
                state.spans[index].start_count += 1;
            } else {
                state.unknown += 1;
            }
        }
        index
    }

    fn entry(&self, index: usize, entering: bool) {
        let mut state = self.0.lock().expect("profile counter lock poisoned");
        // Read the clock under the same lock as the depths. Callback arrival
        // can be reordered across threads; timestamps taken before locking
        // would permit negative intervals.
        let now = state.started.map(|started| started.elapsed().as_nanos());
        let span = &mut state.spans[index];
        if let Some(now) = now {
            span.advance(now);
            span.enter_count += u64::from(entering);
        }
        // Preserve depth outside the window as well: preexisting entries and
        // exits after cutoff must balance, without mutating the saved snapshot.
        if entering {
            span.active += 1;
        } else {
            span.active = span.active.checked_sub(1).expect("unbalanced profile exit");
        }
    }

    fn finish(&self) -> Result<(Vec<serde_json::Value>, serde_json::Value), String> {
        let mut state = self.0.lock().expect("profile counter lock poisoned");
        let elapsed = state
            .started
            .take()
            .ok_or("profile counter window is not active")?
            .elapsed()
            .as_nanos();
        let window = serde_json::json!({
            "start_unix_nanos": state.start_unix_nanos,
            "end_unix_nanos": state.start_unix_nanos + elapsed,
            "elapsed_nanos": elapsed,
        });
        if state.unknown != 0 {
            return Err(format!(
                "profile subscriber observed {} unregistered target spans",
                state.unknown
            ));
        }
        let spans = NAMES
            .iter()
            .zip(&mut state.spans)
            .map(|(name, span)| {
                span.advance(elapsed);
                let elapsed_nanos = u64::try_from(span.elapsed_nanos)
                    .map_err(|_| "profile entered duration overflow")?;
                Ok(serde_json::json!({
                    "name": name,
                    "start_count": span.start_count,
                    "enter_count": span.enter_count,
                    "active_at_start": span.active_at_start,
                    "active_at_end": span.active,
                    "elapsed_nanos": elapsed_nanos,
                }))
            })
            .collect::<Result<_, String>>()?;
        Ok((spans, window))
    }
}

#[derive(Clone, Copy)]
struct SpanIndex(usize);

struct Layer {
    counters: Arc<Counters>,
}

impl<S> tracing_subscriber::Layer<S> for Layer
where
    S: tracing::Subscriber + for<'lookup> tracing_subscriber::registry::LookupSpan<'lookup>,
{
    fn on_new_span(
        &self,
        attributes: &tracing::span::Attributes<'_>,
        id: &tracing::span::Id,
        context: tracing_subscriber::layer::Context<'_, S>,
    ) {
        if let Some(index) = self.counters.start_span(attributes.metadata().name()) {
            context
                .span(id)
                .expect("new profile span exists")
                .extensions_mut()
                .insert(SpanIndex(index));
        }
    }

    fn on_enter(&self, id: &tracing::span::Id, context: tracing_subscriber::layer::Context<'_, S>) {
        if let Some(span) = context.span(id)
            && let Some(index) = span.extensions().get::<SpanIndex>()
        {
            self.counters.entry(index.0, true);
        }
    }

    fn on_exit(&self, id: &tracing::span::Id, context: tracing_subscriber::layer::Context<'_, S>) {
        if let Some(span) = context.span(id)
            && let Some(index) = span.extensions().get::<SpanIndex>()
        {
            self.counters.entry(index.0, false);
        }
    }
}

pub(crate) struct ProfileSpanRecorder {
    output: std::fs::File,
    counters: Arc<Counters>,
}

impl ProfileSpanRecorder {
    pub(crate) fn new(output: std::fs::File) -> Self {
        Self {
            output,
            counters: Arc::new(Counters::default()),
        }
    }

    pub(crate) fn layer<S>(&self) -> impl tracing_subscriber::Layer<S> + use<S>
    where
        S: tracing::Subscriber + for<'lookup> tracing_subscriber::registry::LookupSpan<'lookup>,
    {
        Layer {
            counters: Arc::clone(&self.counters),
        }
    }

    pub(crate) fn begin(&self) -> Result<(), String> {
        self.counters.begin()
    }

    pub(crate) fn finish(&mut self, window: &serde_json::Value) -> Result<(), String> {
        use std::io::Write;
        let (spans, capture_window) = self.counters.finish()?;
        let record = serde_json::json!({
            "schema_version": 4,
            "instrumentation": "authority_v2",
            "measurement": "entered_scope_wall_time_within_capture_window",
            "window": window,
            "capture_window": capture_window,
            "spans": spans,
        });
        serde_json::to_writer(&mut self.output, &record)
            .map_err(|error| format!("cannot encode profile span counters: {error}"))?;
        self.output
            .write_all(b"\n")
            .and_then(|()| self.output.flush())
            .map_err(|error| format!("cannot write profile span counters: {error}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing_subscriber::layer::SubscriberExt;

    #[allow(
        dead_code,
        reason = "Cargo also compiles this test helper for the harness-free benchmark."
    )]
    fn subscriber(counters: &Arc<Counters>) -> impl tracing::Subscriber + Send + Sync {
        use tracing_subscriber::Layer as _;
        // Keep child runtime spans enabled so the registry retains parents,
        // as it does when this layer is composed with Console.
        tracing_subscriber::registry()
            .with(
                Layer {
                    counters: Arc::clone(counters),
                }
                .with_filter(tracing_subscriber::filter::FilterFn::new(
                    |metadata| metadata.target() == "ckb_tx_pool_profile",
                )),
            )
            .with(tracing_subscriber::layer::Identity::new())
    }

    #[test]
    fn recorder_emits_its_own_anchored_monotonic_window() {
        let output = tempfile::tempfile().unwrap();
        let mut reader = output.try_clone().unwrap();
        let mut recorder = ProfileSpanRecorder::new(output);
        let subscriber = tracing_subscriber::registry().with(recorder.layer());
        tracing::subscriber::with_default(subscriber, || {
            recorder.begin().unwrap();
            {
                let _span = tracing::info_span!("tx_pool.stage.resolve").entered();
            }
            recorder
                .finish(&serde_json::json!({"marker": "fixture"}))
                .unwrap();
        });
        use std::io::{Read, Seek};
        reader.rewind().unwrap();
        let mut text = String::new();
        reader.read_to_string(&mut text).unwrap();
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["schema_version"], 4);
        assert_eq!(value["window"]["marker"], "fixture");
        let window = &value["capture_window"];
        assert_eq!(
            window["end_unix_nanos"].as_u64().unwrap()
                - window["start_unix_nanos"].as_u64().unwrap(),
            window["elapsed_nanos"].as_u64().unwrap()
        );
        assert_eq!(value["spans"][9]["enter_count"], 1);
    }

    #[test]
    fn overlapping_entries_integrate_only_entered_intervals() {
        let mut counter = Counter::default();
        for (now, depth_after) in [(10, 1), (20, 2), (30, 1), (40, 0), (70, 1), (80, 0)] {
            counter.advance(now);
            counter.active = depth_after;
        }
        counter.advance(100);
        assert_eq!(counter.elapsed_nanos, 50);
    }

    #[test]
    fn retained_children_and_repeated_entries_do_not_delay_cutoff() {
        let counters = Arc::new(Counters::default());
        tracing::subscriber::with_default(subscriber(&counters), || {
            counters.begin().unwrap();
            let parent =
                tracing::info_span!(target: "ckb_tx_pool_profile", "tx_pool.effects.publish");
            let child = {
                let _entered = parent.enter();
                let _recursive = parent.enter();
                tracing::info_span!("runtime.spawn")
            };
            {
                let _reentered = parent.enter();
            }
            drop(parent);
            let (spans, _) = counters.finish().unwrap();
            assert_eq!(spans[5]["start_count"], 1);
            assert_eq!(spans[5]["enter_count"], 3);
            assert_eq!(spans[5]["active_at_end"], 0);
            assert!(!child.is_disabled());
            drop(child);
            assert!(counters.finish().is_err());
        });
    }

    #[test]
    fn concurrent_entries_cross_cutoff_without_mutating_snapshot() {
        let counters = Arc::new(Counters::default());
        let dispatch = tracing::Dispatch::new(subscriber(&counters));
        tracing::dispatcher::with_default(&dispatch, || {
            let span =
                tracing::info_span!(target: "ckb_tx_pool_profile", "tx_pool.effects.publish");
            let barrier = std::sync::Barrier::new(3);
            std::thread::scope(|scope| {
                for _ in 0..2 {
                    let (span, dispatch, barrier) = (span.clone(), dispatch.clone(), &barrier);
                    scope.spawn(move || {
                        tracing::dispatcher::with_default(&dispatch, || {
                            let _entered = span.enter();
                            barrier.wait();
                            barrier.wait();
                        })
                    });
                }
                barrier.wait();
                counters.begin().unwrap();
                let (snapshot, _) = counters.finish().unwrap();
                assert_eq!(snapshot[5]["active_at_start"], 2);
                assert_eq!(snapshot[5]["active_at_end"], 2);
                assert_eq!(snapshot[5]["start_count"], 0);
                assert_eq!(snapshot[5]["enter_count"], 0);
                barrier.wait();
            });
            assert_eq!(counters.0.lock().unwrap().spans[5].active, 0);
            counters.begin().unwrap();
            let (snapshot, _) = counters.finish().unwrap();
            assert_eq!(snapshot[5]["active_at_start"], 0);
            assert_eq!(snapshot[5]["elapsed_nanos"], 0);
        });
    }
}
