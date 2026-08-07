use std::collections::{BTreeMap, BTreeSet, VecDeque};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct PermitRequestId(pub(super) u8);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct PermitLease(pub(super) u16);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PermitClass {
    Retained,
    Direct,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct PermitRequest {
    pub(super) id: PermitRequestId,
    pub(super) class: PermitClass,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct FairPermitScheduler {
    capacity: u16,
    free: u16,
    wait_limit: u16,
    next_lease: u16,
    waiting: VecDeque<PermitRequest>,
    active: BTreeMap<PermitLease, PermitRequest>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PermitDisposition {
    Granted {
        request: PermitRequest,
        lease: PermitLease,
    },
    Queued(PermitRequestId),
    QueueFull(PermitRequestId),
    Duplicate(PermitRequestId),
    Released {
        request: PermitRequest,
        next: Option<(PermitRequest, PermitLease)>,
    },
    StaleLease(PermitLease),
    CounterExhausted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PermitConfigurationError {
    ZeroCapacity,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PermitInvariantError {
    Capacity,
    QueueBound,
    DuplicateRequest,
    LeaseOrder,
}

impl FairPermitScheduler {
    pub(super) fn new(capacity: u16, wait_limit: u16) -> Result<Self, PermitConfigurationError> {
        if capacity == 0 || wait_limit == 0 {
            return Err(PermitConfigurationError::ZeroCapacity);
        }
        Ok(Self {
            capacity,
            free: capacity,
            wait_limit,
            next_lease: 1,
            waiting: VecDeque::new(),
            active: BTreeMap::new(),
        })
    }

    pub(super) fn request(&mut self, request: PermitRequest) -> PermitDisposition {
        if self.contains_request(request.id) {
            return PermitDisposition::Duplicate(request.id);
        }
        if self.free == 0 {
            if u16::try_from(self.waiting.len())
                .ok()
                .is_none_or(|waiting| waiting >= self.wait_limit)
            {
                return PermitDisposition::QueueFull(request.id);
            }
            self.waiting.push_back(request);
            return PermitDisposition::Queued(request.id);
        }

        let Some((lease, next_lease)) = self.next_lease() else {
            return PermitDisposition::CounterExhausted;
        };
        let Some(free) = self.free.checked_sub(1) else {
            return PermitDisposition::CounterExhausted;
        };
        self.free = free;
        self.next_lease = next_lease;
        self.active.insert(lease, request);
        PermitDisposition::Granted { request, lease }
    }

    pub(super) fn release(&mut self, lease: PermitLease) -> PermitDisposition {
        let Some(request) = self.active.get(&lease).copied() else {
            return PermitDisposition::StaleLease(lease);
        };
        let next_grant = if let Some(waiting) = self.waiting.front().copied() {
            let Some((next_lease, next_counter)) = self.next_lease() else {
                return PermitDisposition::CounterExhausted;
            };
            Some((waiting, next_lease, next_counter))
        } else {
            None
        };
        let Some(free) = self.free.checked_add(1) else {
            return PermitDisposition::CounterExhausted;
        };

        self.active.remove(&lease);
        self.free = free;
        let next = if let Some((waiting, next_lease, next_counter)) = next_grant {
            self.waiting.pop_front();
            let Some(free) = self.free.checked_sub(1) else {
                return PermitDisposition::CounterExhausted;
            };
            self.free = free;
            self.next_lease = next_counter;
            self.active.insert(next_lease, waiting);
            Some((waiting, next_lease))
        } else {
            None
        };
        PermitDisposition::Released { request, next }
    }

    pub(super) fn waiting_position(&self, request: PermitRequestId) -> Option<usize> {
        self.waiting
            .iter()
            .position(|waiting| waiting.id == request)
    }

    pub(super) fn check_invariants(&self) -> Result<(), PermitInvariantError> {
        let active =
            u16::try_from(self.active.len()).map_err(|_| PermitInvariantError::Capacity)?;
        if self.free.checked_add(active) != Some(self.capacity) {
            return Err(PermitInvariantError::Capacity);
        }
        if u16::try_from(self.waiting.len())
            .ok()
            .is_none_or(|waiting| waiting > self.wait_limit)
        {
            return Err(PermitInvariantError::QueueBound);
        }
        if self.active.keys().any(|lease| lease.0 >= self.next_lease) {
            return Err(PermitInvariantError::LeaseOrder);
        }
        let mut requests = BTreeSet::new();
        if self
            .active
            .values()
            .chain(self.waiting.iter())
            .any(|request| !requests.insert(request.id))
        {
            return Err(PermitInvariantError::DuplicateRequest);
        }
        Ok(())
    }

    fn contains_request(&self, request: PermitRequestId) -> bool {
        self.active
            .values()
            .chain(self.waiting.iter())
            .any(|candidate| candidate.id == request)
    }

    fn next_lease(&self) -> Option<(PermitLease, u16)> {
        Some((
            PermitLease(self.next_lease),
            self.next_lease.checked_add(1)?,
        ))
    }
}
