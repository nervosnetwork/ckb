use std::collections::{BTreeMap, BTreeSet, VecDeque};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct PermitRequestId(pub(super) u8);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct PermitDomain(pub(super) u8);

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

/// Move-only ownership of one active retained-compute permit.
#[must_use = "dropping a retained permit token leaks one modeled scheduler slot"]
#[derive(Debug, PartialEq, Eq)]
pub(super) struct RetainedPermitToken {
    domain: PermitDomain,
    request: PermitRequestId,
}

impl RetainedPermitToken {
    pub(super) fn request(&self) -> PermitRequest {
        PermitRequest {
            id: self.request,
            class: PermitClass::Retained,
        }
    }

    pub(super) fn identity(&self) -> (PermitDomain, PermitRequestId) {
        (self.domain, self.request)
    }
}

/// Move-only ownership of one active owner-free Direct permit.
#[must_use = "dropping a direct permit token leaks one modeled scheduler slot"]
#[derive(Debug, PartialEq, Eq)]
pub(super) struct DirectPermitToken {
    domain: PermitDomain,
    request: PermitRequestId,
}

impl DirectPermitToken {
    pub(super) fn request(&self) -> PermitRequest {
        PermitRequest {
            id: self.request,
            class: PermitClass::Direct,
        }
    }
}

/// The fair scheduler returns a class-tagged move-only ownership token. Callers
/// must match the class before handing it to a class-specific protocol; no
/// runtime class flag can be discarded while retaining ownership.
#[must_use = "a permit grant must be transferred or returned to its scheduler"]
#[derive(Debug, PartialEq, Eq)]
pub(super) enum PermitGrant {
    Retained(RetainedPermitToken),
    Direct(DirectPermitToken),
}

impl PermitGrant {
    fn domain(&self) -> PermitDomain {
        match self {
            Self::Retained(token) => token.domain,
            Self::Direct(token) => token.domain,
        }
    }

    pub(super) fn request(&self) -> PermitRequest {
        match self {
            Self::Retained(token) => token.request(),
            Self::Direct(token) => token.request(),
        }
    }

    fn for_request(domain: PermitDomain, request: PermitRequest) -> Self {
        match request.class {
            PermitClass::Retained => Self::Retained(RetainedPermitToken {
                domain,
                request: request.id,
            }),
            PermitClass::Direct => Self::Direct(DirectPermitToken {
                domain,
                request: request.id,
            }),
        }
    }
}

impl From<RetainedPermitToken> for PermitGrant {
    fn from(token: RetainedPermitToken) -> Self {
        Self::Retained(token)
    }
}

impl From<DirectPermitToken> for PermitGrant {
    fn from(token: DirectPermitToken) -> Self {
        Self::Direct(token)
    }
}

// This is the sole permit authority. In particular it is intentionally not
// Clone: copying it would duplicate active grants and free-slot ownership.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct FairPermitScheduler {
    domain: PermitDomain,
    capacity: u16,
    free: u16,
    wait_limit: u16,
    waiting: VecDeque<PermitRequest>,
    active: BTreeMap<PermitRequestId, PermitClass>,
}

#[must_use = "a permit request disposition may contain a linear grant"]
#[derive(Debug, PartialEq, Eq)]
pub(super) enum PermitRequestDisposition {
    Granted { grant: PermitGrant },
    Queued(PermitRequestId),
    QueueFull(PermitRequestId),
    Duplicate(PermitRequestId),
    InvalidSchedulerState(PermitRequestId),
}

#[must_use = "an immediate permit disposition may contain a linear grant"]
#[derive(Debug, PartialEq, Eq)]
pub(super) enum ImmediatePermitDisposition {
    Granted { grant: PermitGrant },
    Unavailable(PermitRequestId),
    Duplicate(PermitRequestId),
    InvalidSchedulerState(PermitRequestId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PermitReleaseError {
    ForeignOrStale(PermitRequestId),
    InvalidSchedulerState(PermitRequestId),
}

#[must_use = "a permit release rejection returns the original linear grant"]
#[derive(Debug, PartialEq, Eq)]
pub(super) enum PermitReleaseDisposition {
    Released {
        request: PermitRequest,
        next: Option<PermitGrant>,
    },
    Rejected {
        error: PermitReleaseError,
        grant: PermitGrant,
    },
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
}

impl FairPermitScheduler {
    pub(super) fn new(
        domain: PermitDomain,
        capacity: u16,
        wait_limit: u16,
    ) -> Result<Self, PermitConfigurationError> {
        if capacity == 0 || wait_limit == 0 {
            return Err(PermitConfigurationError::ZeroCapacity);
        }
        Ok(Self {
            domain,
            capacity,
            free: capacity,
            wait_limit,
            waiting: VecDeque::new(),
            active: BTreeMap::new(),
        })
    }

    pub(super) fn request(&mut self, request: PermitRequest) -> PermitRequestDisposition {
        if self.contains_request(request.id) {
            return PermitRequestDisposition::Duplicate(request.id);
        }
        if self.free == 0 {
            if u16::try_from(self.waiting.len())
                .ok()
                .is_none_or(|waiting| waiting >= self.wait_limit)
            {
                return PermitRequestDisposition::QueueFull(request.id);
            }
            self.waiting.push_back(request);
            return PermitRequestDisposition::Queued(request.id);
        }

        match self.grant(request) {
            Some(grant) => PermitRequestDisposition::Granted { grant },
            None => PermitRequestDisposition::InvalidSchedulerState(request.id),
        }
    }

    /// Attempt an immediate acquisition without joining the fairness queue.
    /// A coordinator may use this only after its one queued acquisition has
    /// been granted, so a worker wave cannot manufacture one waiter per idle
    /// slot or bypass an older Direct request.
    pub(super) fn try_request(&mut self, request: PermitRequest) -> ImmediatePermitDisposition {
        if self.contains_request(request.id) {
            return ImmediatePermitDisposition::Duplicate(request.id);
        }
        if self.free == 0 {
            return ImmediatePermitDisposition::Unavailable(request.id);
        }
        match self.grant(request) {
            Some(grant) => ImmediatePermitDisposition::Granted { grant },
            None => ImmediatePermitDisposition::InvalidSchedulerState(request.id),
        }
    }

    pub(super) fn release(&mut self, grant: PermitGrant) -> PermitReleaseDisposition {
        let domain = grant.domain();
        let request = grant.request();
        if domain != self.domain || self.active.get(&request.id).copied() != Some(request.class) {
            return PermitReleaseDisposition::Rejected {
                error: PermitReleaseError::ForeignOrStale(request.id),
                grant,
            };
        };
        let next = if let Some(waiting) = self.waiting.pop_front() {
            self.active.remove(&request.id);
            self.active.insert(waiting.id, waiting.class);
            Some(PermitGrant::for_request(self.domain, waiting))
        } else {
            let Some(free) = self.free.checked_add(1) else {
                return PermitReleaseDisposition::Rejected {
                    error: PermitReleaseError::InvalidSchedulerState(request.id),
                    grant,
                };
            };
            self.active.remove(&request.id);
            self.free = free;
            None
        };
        PermitReleaseDisposition::Released { request, next }
    }

    pub(super) fn waiting_position(&self, request: PermitRequestId) -> Option<usize> {
        self.waiting
            .iter()
            .position(|waiting| waiting.id == request)
    }

    pub(super) fn active_request(&self, request: PermitRequestId) -> Option<PermitRequest> {
        self.active
            .get(&request)
            .copied()
            .map(|class| PermitRequest { id: request, class })
    }

    pub(super) fn owns_retained(&self, token: &RetainedPermitToken) -> bool {
        token.domain == self.domain && self.active_request(token.request) == Some(token.request())
    }

    pub(super) fn is_active(&self, request: PermitRequestId) -> bool {
        self.active.contains_key(&request)
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
        let mut requests = BTreeSet::new();
        if self
            .active
            .keys()
            .copied()
            .chain(self.waiting.iter().map(|request| request.id))
            .any(|request| !requests.insert(request))
        {
            return Err(PermitInvariantError::DuplicateRequest);
        }
        Ok(())
    }

    fn contains_request(&self, request: PermitRequestId) -> bool {
        self.active.contains_key(&request)
            || self.waiting.iter().any(|candidate| candidate.id == request)
    }

    fn grant(&mut self, request: PermitRequest) -> Option<PermitGrant> {
        let free = self.free.checked_sub(1)?;
        self.free = free;
        self.active.insert(request.id, request.class);
        Some(PermitGrant::for_request(self.domain, request))
    }
}
