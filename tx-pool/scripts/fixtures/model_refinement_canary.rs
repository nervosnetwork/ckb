// Parser and negative-binding canary for check_model_refinement.py.
// This file is data for the checker; it is not compiled into ckb-tx-pool.

pub(crate) struct CanaryPayload(pub(crate) u8);

pub(crate) enum CanaryEvent {
    Tuple(CanaryPayload),
    Struct { payload: CanaryPayload },
    Unit,
}

pub(crate) struct CanaryBoundary {
    pub(crate) event: CanaryEvent,
}

pub(crate) struct CanaryFreeFunctionPayload;

pub(crate) struct CanaryEvidencePayload;

pub(crate) struct CanaryUnregisteredEvidencePayload;

pub(crate) struct CanaryUnconstructedCapability;

pub enum CanaryExternalEvent {
    External,
}

pub(crate) fn canary_behavior_entry() -> CanaryBoundary {
    CanaryBoundary::new(CanaryEvent::unit(), [])
}

fn canary_root_payload() -> CanaryFreeFunctionPayload {
    CanaryFreeFunctionPayload
}

#[test]
fn canary_registered_evidence() {
    let _payload = CanaryEvidencePayload;
}

#[test]
fn canary_unregistered_evidence() {
    let _payload = CanaryUnregisteredEvidencePayload;
}

impl CanaryBoundary {
    pub(crate) fn new(
        event: CanaryEvent,
        _additional: impl IntoIterator<Item = CanaryEvent>,
    ) -> Self {
        Self { event }
    }
}

impl CanaryEvent {
    pub(crate) fn tuple(value: u8) -> Self {
        Self::Tuple(CanaryPayload(value))
    }

    pub(crate) fn structured(value: u8) -> Self {
        Self::Struct {
            payload: CanaryPayload(value),
        }
    }

    pub(crate) fn unit() -> Self {
        Self::Unit
    }

    pub(crate) fn consume(self) -> u8 {
        match self {
            Self::Tuple(CanaryPayload(value))
            | Self::Struct {
                payload: CanaryPayload(value),
            } => value,
            Self::Unit => {
                let _payload = canary_root_payload();
                0
            }
        }
    }
}
