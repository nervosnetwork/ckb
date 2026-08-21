//! Stable-cut vocabulary for one production lifecycle property.

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct TraceTxId(pub(crate) u8);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct TracePeerId(pub(crate) u8);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum TraceVerifyClass {
    Small,
    Large,
}

impl TraceVerifyClass {
    pub(crate) const ALL: [Self; 2] = [Self::Small, Self::Large];
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum TraceVerifyCapability {
    Any,
    SmallCycleOnly,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum TraceWorkPermit {
    ResolveOnly,
    VerifyOnly(TraceVerifyCapability),
    ResolveThenVerify(TraceVerifyCapability),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum TraceLifecycleRoute {
    Split,
    Continuous(TraceVerifyCapability),
}

impl TraceLifecycleRoute {
    pub(crate) const ALL: [Self; 3] = [
        Self::Split,
        Self::Continuous(TraceVerifyCapability::Any),
        Self::Continuous(TraceVerifyCapability::SmallCycleOnly),
    ];

    const fn initial_permit(self) -> TraceWorkPermit {
        match self {
            Self::Split => TraceWorkPermit::ResolveOnly,
            Self::Continuous(capability) => TraceWorkPermit::ResolveThenVerify(capability),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum TraceAction {
    AdmitRemote {
        transaction: TraceTxId,
        peer: TracePeerId,
        deadline: u64,
    },
    Checkout(TraceWorkPermit),
    Resolve(TraceTxId),
    Verify(TraceTxId),
    FinalizeReady,
    ClaimEffect,
    SettleEffect,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct TraceTransaction {
    pub(crate) id: TraceTxId,
    pub(crate) verify_class: TraceVerifyClass,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TraceScenario {
    pub(crate) transaction: TraceTransaction,
    pub(crate) peer: TracePeerId,
    pub(crate) route: TraceLifecycleRoute,
}

impl TraceScenario {
    pub(crate) fn lifecycle(
        transaction: TraceTxId,
        verify_class: TraceVerifyClass,
        route: TraceLifecycleRoute,
    ) -> Self {
        Self {
            transaction: TraceTransaction {
                id: transaction,
                verify_class,
            },
            peer: TracePeerId(1),
            route,
        }
    }

    /// The action shape is intentionally independent of route/class outcome.
    /// A compatible continuous route observes an idle Verify checkout, while a
    /// split or incompatible route observes a real checkout at the same cut.
    pub(crate) fn actions(&self) -> Vec<TraceAction> {
        vec![
            TraceAction::AdmitRemote {
                transaction: self.transaction.id,
                peer: self.peer,
                deadline: 100,
            },
            TraceAction::Checkout(self.route.initial_permit()),
            TraceAction::Resolve(self.transaction.id),
            TraceAction::Checkout(TraceWorkPermit::VerifyOnly(TraceVerifyCapability::Any)),
            TraceAction::Verify(self.transaction.id),
            TraceAction::FinalizeReady,
            TraceAction::ClaimEffect,
            TraceAction::SettleEffect,
        ]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum TraceWorkStage {
    Resolve,
    Verify(TraceVerifyClass),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum TraceAcceptedStatus {
    Pending,
    Gap,
    Proposed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum TraceRetainedSource {
    Remote(TracePeerId),
    Proposal { ingress_peer: Option<TracePeerId> },
    Recovery,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum TraceAcceptedProvenance {
    Trusted,
    Peer(TracePeerId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum TraceRetainedPhase {
    QueuedResolve,
    QueuedVerify(TraceVerifyClass),
    Computing(TraceWorkPermit),
    Waiting,
    Ready,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum TraceOwnerLocation {
    Retained {
        source: TraceRetainedSource,
        phase: TraceRetainedPhase,
    },
    Accepted {
        provenance: TraceAcceptedProvenance,
        status: TraceAcceptedStatus,
    },
    ReplacementHistory,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct TraceOwnerObservation {
    pub(crate) transaction: TraceTxId,
    pub(crate) version_rank: u16,
    pub(crate) arrival_rank: u16,
    pub(crate) location: TraceOwnerLocation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum TraceWorkLocation {
    Executing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct TraceWorkObservation {
    pub(crate) capability_rank: u16,
    pub(crate) transaction: TraceTxId,
    pub(crate) permit: TraceWorkPermit,
    pub(crate) stage: TraceWorkStage,
    pub(crate) location: TraceWorkLocation,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) struct TraceResourceCounts {
    pub(crate) owners: u16,
    pub(crate) charged_owners: u16,
    pub(crate) retained: u16,
    pub(crate) remote: u16,
    pub(crate) accepted: u16,
    pub(crate) replacement_history: u16,
    pub(crate) active_work: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum TraceEffectClass {
    Remote,
    Trusted,
    Critical,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum TraceEffect {
    Accepted {
        transaction: TraceTxId,
        status: TraceAcceptedStatus,
        ingress_peer: Option<TracePeerId>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct TraceEffectObservation {
    pub(crate) sequence: u64,
    pub(crate) ordinal: u16,
    pub(crate) class: TraceEffectClass,
    pub(crate) effect: TraceEffect,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct TraceEffectClaim {
    pub(crate) sequence: u64,
    pub(crate) ordinal: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TraceObservation {
    pub(crate) last_apply: u64,
    pub(crate) generation: u64,
    pub(crate) chain_revision: u64,
    pub(crate) owners: Vec<TraceOwnerObservation>,
    pub(crate) work: Vec<TraceWorkObservation>,
    pub(crate) resources: TraceResourceCounts,
    pub(crate) effects: Vec<TraceEffectObservation>,
    pub(crate) effect_claim: Option<TraceEffectClaim>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum TraceDisposition {
    Retained(TraceTxId),
    CheckedOut {
        transaction: TraceTxId,
        permit: TraceWorkPermit,
        stage: TraceWorkStage,
    },
    ResolveContinued(TraceTxId),
    QueuedVerify(TraceTxId),
    Ready(TraceTxId),
    Accepted(TraceTxId),
    EffectClaimed(TraceEffectClaim),
    EffectSettled(TraceEffectClaim),
    Idle,
}
