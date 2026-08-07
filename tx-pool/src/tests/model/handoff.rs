use super::{
    protocol::RequestId,
    state::{CapabilityId, PeerId, TxId, WitnessId},
};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct RelayItem {
    pub(super) raw: TxId,
    pub(super) witness: WitnessId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RelaySource {
    Remote(PeerId),
    Proposal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RelayLocation {
    CallerOwned,
    Queued(RequestId),
    HandlerOwned(RequestId),
    AuthorityOwned,
    SettledKnown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct RelayRecord {
    pub(super) item: RelayItem,
    pub(super) source: RelaySource,
    pub(super) bytes: u32,
    pub(super) location: RelayLocation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct RelayLimits {
    pub(super) records: u16,
    pub(super) bytes: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct RelayHandoff {
    pub(super) records: BTreeMap<TxId, RelayRecord>,
    pub(super) limits: RelayLimits,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RelayTerminal {
    Accepted,
    Rejected,
    UnknownParents,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RelayDisposition {
    Offered(RelayItem),
    Duplicate(RelayItem),
    PayloadVariant(RelayItem),
    ResourceRejected(RelayItem),
    Enqueued(RelayItem),
    Dispatched(RelayItem),
    AuthorityAccepted(RelayItem),
    KnownSettled(RelayItem),
    Released(RelayItem),
    Forgotten(RelayItem),
    Unavailable(RelayItem),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RelayInvariantError {
    RawIdentityMismatch,
    RecordBound,
    ByteOverflow,
    ByteBound,
}

impl RelayHandoff {
    pub(super) fn new(limits: RelayLimits) -> Self {
        Self {
            records: BTreeMap::new(),
            limits,
        }
    }

    pub(super) fn offer(
        &mut self,
        item: RelayItem,
        source: RelaySource,
        bytes: u32,
    ) -> RelayDisposition {
        if let Some(existing) = self.records.get(&item.raw) {
            return if existing.item == item {
                RelayDisposition::Duplicate(item)
            } else {
                RelayDisposition::PayloadVariant(item)
            };
        }
        let records = u16::try_from(self.records.len())
            .ok()
            .and_then(|used| used.checked_add(1));
        let total_bytes = self.used_bytes().and_then(|used| used.checked_add(bytes));
        if records.is_none_or(|records| records > self.limits.records)
            || total_bytes.is_none_or(|bytes| bytes > self.limits.bytes)
        {
            return RelayDisposition::ResourceRejected(item);
        }
        self.records.insert(
            item.raw,
            RelayRecord {
                item,
                source,
                bytes,
                location: RelayLocation::CallerOwned,
            },
        );
        RelayDisposition::Offered(item)
    }

    pub(super) fn enqueue(
        &mut self,
        item: RelayItem,
        request: RequestId,
        accepted: bool,
    ) -> RelayDisposition {
        let Some(record) = self.records.get_mut(&item.raw) else {
            return RelayDisposition::Unavailable(item);
        };
        if record.item != item || record.location != RelayLocation::CallerOwned {
            return RelayDisposition::Unavailable(item);
        }
        if !accepted {
            self.records.remove(&item.raw);
            return RelayDisposition::Released(item);
        }
        record.location = RelayLocation::Queued(request);
        RelayDisposition::Enqueued(item)
    }

    pub(super) fn dispatch(&mut self, item: RelayItem, request: RequestId) -> RelayDisposition {
        let Some(record) = self.records.get_mut(&item.raw) else {
            return RelayDisposition::Unavailable(item);
        };
        if record.item != item || record.location != RelayLocation::Queued(request) {
            return RelayDisposition::Unavailable(item);
        }
        record.location = RelayLocation::HandlerOwned(request);
        RelayDisposition::Dispatched(item)
    }

    pub(super) fn authority_accept(
        &mut self,
        item: RelayItem,
        request: RequestId,
    ) -> RelayDisposition {
        let Some(record) = self.records.get_mut(&item.raw) else {
            return RelayDisposition::Unavailable(item);
        };
        if record.item != item || record.location != RelayLocation::HandlerOwned(request) {
            return RelayDisposition::Unavailable(item);
        }
        record.location = RelayLocation::AuthorityOwned;
        RelayDisposition::AuthorityAccepted(item)
    }

    pub(super) fn abort_request(&mut self, request: RequestId) -> Vec<RelayItem> {
        let released = self
            .records
            .values()
            .filter_map(|record| {
                matches!(
                    record.location,
                    RelayLocation::Queued(owner) | RelayLocation::HandlerOwned(owner)
                        if owner == request
                )
                .then_some(record.item)
            })
            .collect::<Vec<_>>();
        for item in &released {
            self.records.remove(&item.raw);
        }
        released
    }

    /// Revoke only this peer's pre-authority handoffs. Authority-owned and
    /// settled-known items are left to their owning authority/publication
    /// transitions, so a ban cannot erase an already applied transaction.
    pub(super) fn revoke_peer_before_authority(&mut self, peer: PeerId) -> Vec<RelayItem> {
        let released = self
            .records
            .values()
            .filter_map(|record| {
                (record.source == RelaySource::Remote(peer)
                    && matches!(
                        record.location,
                        RelayLocation::CallerOwned
                            | RelayLocation::Queued(_)
                            | RelayLocation::HandlerOwned(_)
                    ))
                .then_some(record.item)
            })
            .collect::<Vec<_>>();
        for item in &released {
            self.records.remove(&item.raw);
        }
        released
    }

    pub(super) fn settle(&mut self, item: RelayItem, terminal: RelayTerminal) -> RelayDisposition {
        let Some(record) = self.records.get_mut(&item.raw) else {
            return RelayDisposition::Unavailable(item);
        };
        if record.item != item || record.location != RelayLocation::AuthorityOwned {
            return RelayDisposition::Unavailable(item);
        }
        if terminal == RelayTerminal::Accepted {
            record.location = RelayLocation::SettledKnown;
            RelayDisposition::KnownSettled(item)
        } else {
            self.records.remove(&item.raw);
            RelayDisposition::Released(item)
        }
    }

    pub(super) fn forget(&mut self, item: RelayItem) -> RelayDisposition {
        if self.records.get(&item.raw).is_some_and(|record| {
            record.item == item && record.location == RelayLocation::SettledKnown
        }) {
            self.records.remove(&item.raw);
            RelayDisposition::Forgotten(item)
        } else {
            RelayDisposition::Unavailable(item)
        }
    }

    pub(super) fn check_invariants(&self) -> Result<(), RelayInvariantError> {
        if self
            .records
            .iter()
            .any(|(raw, record)| *raw != record.item.raw)
        {
            return Err(RelayInvariantError::RawIdentityMismatch);
        }
        let records =
            u16::try_from(self.records.len()).map_err(|_| RelayInvariantError::RecordBound)?;
        if records > self.limits.records {
            return Err(RelayInvariantError::RecordBound);
        }
        let bytes = self.used_bytes().ok_or(RelayInvariantError::ByteOverflow)?;
        if bytes > self.limits.bytes {
            return Err(RelayInvariantError::ByteBound);
        }
        Ok(())
    }

    fn used_bytes(&self) -> Option<u32> {
        self.records
            .values()
            .try_fold(0u32, |total, record| total.checked_add(record.bytes))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum EndpointCircuit {
    Available,
    DetachedOne,
    Disabled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum EndpointEvent {
    CallReturned,
    CallTimedOut,
    DetachedReturned,
    Disable,
}

impl EndpointCircuit {
    pub(super) fn step(self, event: EndpointEvent) -> Self {
        match (self, event) {
            (Self::Available, EndpointEvent::CallReturned) => Self::Available,
            (Self::Available, EndpointEvent::CallTimedOut) => Self::DetachedOne,
            (Self::DetachedOne, EndpointEvent::DetachedReturned) => Self::Disabled,
            (_, EndpointEvent::Disable) => Self::Disabled,
            (state, _) => state,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct CapabilityTransport {
    capability: CapabilityId,
    location: CapabilityTransportLocation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CapabilityTransportLocation {
    Coordinator,
    AssignmentChannel,
    Worker,
    Returned,
    Terminal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CapabilityTransportDisposition {
    AssignmentSent(CapabilityId),
    AssignmentReturned(CapabilityId),
    WorkerReceived(CapabilityId),
    CompletionDelivered(CapabilityId),
    CompletionReturned(CapabilityId),
    Settled(CapabilityId),
    Unavailable(CapabilityId),
}

impl CapabilityTransport {
    pub(super) const fn new(capability: CapabilityId) -> Self {
        Self {
            capability,
            location: CapabilityTransportLocation::Coordinator,
        }
    }

    pub(super) fn send_assignment(
        mut self,
        accepted: bool,
    ) -> (Self, CapabilityTransportDisposition) {
        if self.location != CapabilityTransportLocation::Coordinator {
            let capability = self.capability;
            return (
                self,
                CapabilityTransportDisposition::Unavailable(capability),
            );
        }
        let disposition = if accepted {
            self.location = CapabilityTransportLocation::AssignmentChannel;
            CapabilityTransportDisposition::AssignmentSent(self.capability)
        } else {
            self.location = CapabilityTransportLocation::Returned;
            CapabilityTransportDisposition::AssignmentReturned(self.capability)
        };
        (self, disposition)
    }

    pub(super) fn receive_assignment(mut self) -> (Self, CapabilityTransportDisposition) {
        if self.location != CapabilityTransportLocation::AssignmentChannel {
            let capability = self.capability;
            return (
                self,
                CapabilityTransportDisposition::Unavailable(capability),
            );
        }
        self.location = CapabilityTransportLocation::Worker;
        let capability = self.capability;
        (
            self,
            CapabilityTransportDisposition::WorkerReceived(capability),
        )
    }

    pub(super) fn send_completion(
        mut self,
        accepted: bool,
    ) -> (Self, CapabilityTransportDisposition) {
        if self.location != CapabilityTransportLocation::Worker {
            let capability = self.capability;
            return (
                self,
                CapabilityTransportDisposition::Unavailable(capability),
            );
        }
        self.location = if accepted {
            CapabilityTransportLocation::Coordinator
        } else {
            CapabilityTransportLocation::Returned
        };
        let disposition = if accepted {
            CapabilityTransportDisposition::CompletionDelivered(self.capability)
        } else {
            CapabilityTransportDisposition::CompletionReturned(self.capability)
        };
        (self, disposition)
    }

    pub(super) fn settle(mut self) -> (Self, CapabilityTransportDisposition) {
        if !matches!(
            self.location,
            CapabilityTransportLocation::Coordinator | CapabilityTransportLocation::Returned
        ) {
            let capability = self.capability;
            return (
                self,
                CapabilityTransportDisposition::Unavailable(capability),
            );
        }
        self.location = CapabilityTransportLocation::Terminal;
        let capability = self.capability;
        (self, CapabilityTransportDisposition::Settled(capability))
    }

    pub(super) const fn is_terminal(&self) -> bool {
        matches!(self.location, CapabilityTransportLocation::Terminal)
    }
}
