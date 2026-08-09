//! Finite reference relations for post-commit wake and effect publication.
//!
//! These values are observations of one authoritative before/after cut. They
//! never select work and never become a second runtime level owner.

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct ProgressVersion(pub(crate) u128);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct SchedulerProgressCut {
    pub(crate) resolve: Option<ProgressVersion>,
    pub(crate) verify_small: Option<ProgressVersion>,
    pub(crate) verify_any: Option<ProgressVersion>,
    pub(crate) ready: Option<ProgressVersion>,
}

impl SchedulerProgressCut {
    fn has_compute_head(self) -> bool {
        self.resolve.is_some() || self.verify_small.is_some() || self.verify_any.is_some()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct EffectUsageCut {
    pub(crate) remote_batches: usize,
    pub(crate) remote_bytes: usize,
    pub(crate) ordinary_batches: usize,
    pub(crate) ordinary_bytes: usize,
    pub(crate) total_batches: usize,
    pub(crate) total_bytes: usize,
}

impl EffectUsageCut {
    fn released_from(self, before: Self) -> bool {
        self.remote_batches < before.remote_batches
            || self.remote_bytes < before.remote_bytes
            || self.ordinary_batches < before.ordinary_batches
            || self.ordinary_bytes < before.ordinary_bytes
            || self.total_batches < before.total_batches
            || self.total_bytes < before.total_bytes
    }

    fn is_empty(self) -> bool {
        self == Self::default()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct EffectHead {
    pub(crate) sequence: u128,
    pub(crate) ordinal: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EffectReceiptSource {
    Queued,
    GenerationReset,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct EffectLogCut {
    pub(crate) queued: Option<EffectHead>,
    pub(crate) generation_reset: Option<EffectHead>,
    pub(crate) closed: bool,
    pub(crate) pending_recent_rejects: usize,
    pub(crate) usage: EffectUsageCut,
}

impl Default for EffectLogCut {
    fn default() -> Self {
        Self {
            queued: None,
            generation_reset: None,
            closed: false,
            pending_recent_rejects: 0,
            usage: EffectUsageCut::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EffectPublicationObservation {
    Receipt {
        source: EffectReceiptSource,
        head: EffectHead,
    },
    Idle,
    ClosedAndDrained,
}

impl EffectLogCut {
    /// Derive the sole publisher's complete observation from one coherent log
    /// cut. A queued record wins an impossible equal-sequence tie, matching
    /// the production ordering relation without weakening sequence uniqueness.
    pub(crate) fn publication_observation(self) -> EffectPublicationObservation {
        let receipt = match (self.queued, self.generation_reset) {
            (Some(queued), Some(reset)) if reset.sequence < queued.sequence => {
                Some((EffectReceiptSource::GenerationReset, reset))
            }
            (Some(queued), _) => Some((EffectReceiptSource::Queued, queued)),
            (None, Some(reset)) => Some((EffectReceiptSource::GenerationReset, reset)),
            (None, None) => None,
        };
        if let Some((source, head)) = receipt {
            return EffectPublicationObservation::Receipt { source, head };
        }
        if self.closed && self.pending_recent_rejects == 0 && self.usage.is_empty() {
            EffectPublicationObservation::ClosedAndDrained
        } else {
            EffectPublicationObservation::Idle
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EffectPublisherLevel {
    Idle,
    Available,
    ClosedAndDrained,
}

impl EffectLogCut {
    fn publisher_level(self) -> EffectPublisherLevel {
        match self.publication_observation() {
            EffectPublicationObservation::Receipt { .. } => EffectPublisherLevel::Available,
            EffectPublicationObservation::Idle => EffectPublisherLevel::Idle,
            EffectPublicationObservation::ClosedAndDrained => {
                EffectPublisherLevel::ClosedAndDrained
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EffectWaitDisposition {
    Publish {
        source: EffectReceiptSource,
        head: EffectHead,
    },
    WaitForProducerCommit,
    Terminate,
}

impl EffectPublicationObservation {
    pub(crate) const fn wait_disposition(self) -> EffectWaitDisposition {
        match self {
            Self::Receipt { source, head } => EffectWaitDisposition::Publish { source, head },
            Self::Idle => EffectWaitDisposition::WaitForProducerCommit,
            Self::ClosedAndDrained => EffectWaitDisposition::Terminate,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AuthorityProgressCut {
    pub(crate) scheduler: SchedulerProgressCut,
    pub(crate) active_work: usize,
    pub(crate) dependency_maintenance: bool,
    pub(crate) effects: EffectLogCut,
    pub(crate) template_sources: [u128; 5],
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct WakeObservation {
    pub(crate) compute: bool,
    pub(crate) ready: bool,
    pub(crate) dependency_maintenance: bool,
    pub(crate) effect_publisher: bool,
    pub(crate) effect_capacity: bool,
    pub(crate) template: bool,
}

impl WakeObservation {
    pub(crate) fn between(before: AuthorityProgressCut, after: AuthorityProgressCut) -> Self {
        let compute_head_advanced =
            head_advanced(before.scheduler.resolve, after.scheduler.resolve)
                || head_advanced(before.scheduler.verify_small, after.scheduler.verify_small)
                || head_advanced(before.scheduler.verify_any, after.scheduler.verify_any);
        let compute_slot_released = after.active_work < before.active_work;
        let before_publisher = before.effects.publisher_level();
        let after_publisher = after.effects.publisher_level();
        Self {
            compute: compute_head_advanced
                || (compute_slot_released && after.scheduler.has_compute_head()),
            ready: head_advanced(before.scheduler.ready, after.scheduler.ready),
            dependency_maintenance: !before.dependency_maintenance && after.dependency_maintenance,
            effect_publisher: after_publisher != EffectPublisherLevel::Idle
                && after_publisher != before_publisher,
            effect_capacity: after.effects.usage.released_from(before.effects.usage),
            template: before.template_sources != after.template_sources,
        }
    }

    /// Each true field emits exactly one Notify operation. Broadcast fanout is
    /// bounded separately by the fixed task/capability topology.
    pub(crate) const fn notification_operations(self) -> u8 {
        self.compute as u8
            + self.ready as u8
            + self.dependency_maintenance as u8
            + self.effect_publisher as u8
            + self.effect_capacity as u8
            + self.template as u8
    }
}

fn head_advanced(before: Option<ProgressVersion>, after: Option<ProgressVersion>) -> bool {
    after.is_some() && before != after
}
