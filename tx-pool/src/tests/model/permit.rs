use super::state::{VerifyCapability, WorkPermit};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct PermitRequestId(pub(super) u8);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct PermitDomain(pub(super) u8);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct WorkerSlotId(pub(super) u8);

/// Stable execution role of one retained worker slot. This is ephemeral
/// topology evidence, never transaction-owner state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RetainedWorkerRole {
    OrderedResolve,
    Verifier(VerifyCapability),
}

impl RetainedWorkerRole {
    pub(super) const fn resolve_permit(self) -> WorkPermit {
        match self {
            Self::OrderedResolve => WorkPermit::ResolveOnly,
            Self::Verifier(capability) => WorkPermit::ResolveThenVerify(capability),
        }
    }

    pub(super) const fn verify_permit(self) -> Option<WorkPermit> {
        match self {
            Self::OrderedResolve => None,
            Self::Verifier(capability) => Some(WorkPermit::VerifyOnly(capability)),
        }
    }

    const fn canonical_rank(self) -> u8 {
        match self {
            Self::OrderedResolve => 0,
            Self::Verifier(VerifyCapability::SmallCycleOnly) => 1,
            Self::Verifier(VerifyCapability::Any) => 2,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct RetainedWorkerSlot {
    id: WorkerSlotId,
    role: RetainedWorkerRole,
}

impl RetainedWorkerSlot {
    pub(super) const fn new(id: WorkerSlotId, role: RetainedWorkerRole) -> Self {
        Self { id, role }
    }

    pub(super) const fn id(self) -> WorkerSlotId {
        self.id
    }

    pub(super) const fn role(self) -> RetainedWorkerRole {
        self.role
    }

    pub(super) const fn canonical_key(self) -> (u8, WorkerSlotId) {
        (self.role.canonical_rank(), self.id)
    }
}

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

/// One fair count permit paired with one unique idle retained-worker slot.
/// The pair still carries no transaction identity; only the scheduler
/// quotient may turn it into an exact checkout assignment.
#[must_use = "a worker grant must be assigned or returned with its permit"]
#[derive(Debug, PartialEq, Eq)]
pub(super) struct RetainedWorkerGrant {
    permit: RetainedPermitToken,
    slot: RetainedWorkerSlot,
}

impl RetainedWorkerGrant {
    pub(super) fn request(&self) -> PermitRequest {
        self.permit.request()
    }

    pub(super) const fn slot(&self) -> RetainedWorkerSlot {
        self.slot
    }

    pub(super) fn into_permit(self) -> RetainedPermitToken {
        self.permit
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum WorkerGrantBatchErrorKind {
    CountMismatch { permits: usize, slots: usize },
    DuplicateWorkerSlot(WorkerSlotId),
    MultipleOrderedResolvers,
}

/// Binding failure returns both independently owned resource sets.
#[must_use = "a rejected worker binding still owns every permit and worker slot"]
#[derive(Debug, PartialEq, Eq)]
pub(super) struct WorkerGrantBatchError {
    pub(super) kind: WorkerGrantBatchErrorKind,
    permits: super::composition::RetainedPermitGrant,
    slots: Vec<RetainedWorkerSlot>,
}

impl WorkerGrantBatchError {
    pub(super) fn into_parts(
        self,
    ) -> (
        super::composition::RetainedPermitGrant,
        Vec<RetainedWorkerSlot>,
    ) {
        (self.permits, self.slots)
    }
}

/// Role-bearing grant batch accepted by the compute exchange. Construction
/// consumes a previously validated same-domain count batch and a unique idle
/// worker-slot set, so count-only evidence cannot authorize assignment.
#[must_use = "a worker grant batch owns every fair permit and idle worker slot"]
#[derive(Debug, PartialEq, Eq)]
pub(super) struct RetainedWorkerGrantBatch {
    grants: Vec<RetainedWorkerGrant>,
}

impl RetainedWorkerGrantBatch {
    pub(super) fn bind(
        permits: super::composition::RetainedPermitGrant,
        mut slots: Vec<RetainedWorkerSlot>,
    ) -> Result<Self, WorkerGrantBatchError> {
        let permit_count = permits.request_ids().len();
        if permit_count != slots.len() {
            return Err(WorkerGrantBatchError {
                kind: WorkerGrantBatchErrorKind::CountMismatch {
                    permits: permit_count,
                    slots: slots.len(),
                },
                permits,
                slots,
            });
        }
        slots.sort_unstable_by_key(|slot| slot.canonical_key());
        let mut slot_ids = BTreeSet::new();
        if let Some(duplicate) = slots
            .iter()
            .map(|slot| slot.id())
            .find(|slot| !slot_ids.insert(*slot))
        {
            return Err(WorkerGrantBatchError {
                kind: WorkerGrantBatchErrorKind::DuplicateWorkerSlot(duplicate),
                permits,
                slots,
            });
        }
        if slots
            .iter()
            .filter(|slot| slot.role() == RetainedWorkerRole::OrderedResolve)
            .count()
            > 1
        {
            return Err(WorkerGrantBatchError {
                kind: WorkerGrantBatchErrorKind::MultipleOrderedResolvers,
                permits,
                slots,
            });
        }
        let grants = permits
            .into_tokens()
            .into_iter()
            .zip(slots)
            .map(|(permit, slot)| RetainedWorkerGrant { permit, slot })
            .collect();
        Ok(Self { grants })
    }

    pub(super) fn empty() -> Self {
        Self { grants: Vec::new() }
    }

    pub(super) fn request_ids(&self) -> BTreeSet<PermitRequestId> {
        self.grants.iter().map(|grant| grant.request().id).collect()
    }

    pub(super) fn into_grants(self) -> Vec<RetainedWorkerGrant> {
        self.grants
    }

    /// Reunite grants obtained by destructuring one previously validated
    /// batch. `RetainedWorkerGrant` has no public constructor, so this cannot
    /// admit a new domain, permit identity or worker slot.
    pub(super) fn reunite(grants: Vec<RetainedWorkerGrant>) -> Self {
        Self { grants }
    }
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
