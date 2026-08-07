//! Composed reference trace for protocol boundaries outside the authority cut.
//!
//! This adapter delegates all state transitions to the existing controller,
//! relay-handoff, kernel and endpoint automata. It owns only the neutral
//! checkpoints used to compose independently replayed production boundaries.

use super::{
    handoff::{
        EndpointCircuit, EndpointEvent, RelayDisposition, RelayHandoff, RelayItem, RelayLimits,
        RelayLocation, RelaySource, RelayTerminal,
    },
    kernel::{KernelCommand, KernelDisposition, KernelStep},
    protocol::{
        KernelAccess, Lifecycle, PayloadCost, PayloadLocation, ProtocolLimits, RequestId,
        RequestKind, ResponseEndpoint, ResponseResult, SystemDisposition, SystemEvent, SystemState,
    },
    state::{ModelLimits, MonotonicTick, PeerId, RulesId, TxId, ViewId, WitnessId},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct BoundaryTxId(pub(crate) u8);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct BoundaryWitnessId(pub(crate) u8);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct BoundaryPeerId(pub(crate) u8);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct BoundaryRequestId(pub(crate) u8);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BoundarySource {
    Remote(BoundaryPeerId),
    Proposal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct BoundaryKey {
    pub(crate) raw: BoundaryTxId,
    pub(crate) witness: BoundaryWitnessId,
    pub(crate) source: BoundarySource,
    pub(crate) request: BoundaryRequestId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BoundaryLifecycleState {
    Constructing,
    Initializing,
    Running,
    Draining,
    Stopped,
    StartupFailed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BoundaryControllerState {
    Queued,
    HandlerOwned,
    ResponseSent,
    NotificationFinished,
    Full,
    Closed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BoundaryHandoffState {
    CallerOwned,
    Queued,
    HandlerOwned,
    AuthorityOwned,
    Released,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BoundaryEffectState {
    Committed,
    Claimed,
    Settled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BoundaryRelaySettlement {
    ExactRelease,
    ConservativeReset,
    CircuitDisposed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BoundaryEnqueueFailure {
    Full,
    Closed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BoundaryCheckpoint {
    Lifecycle(BoundaryLifecycleState),
    Controller {
        key: BoundaryKey,
        state: BoundaryControllerState,
    },
    Handoff {
        key: BoundaryKey,
        state: BoundaryHandoffState,
    },
    AuthorityRejected {
        key: BoundaryKey,
    },
    Effect {
        key: BoundaryKey,
        state: BoundaryEffectState,
    },
    Relay {
        key: BoundaryKey,
        settlement: BoundaryRelaySettlement,
    },
}

struct ReferenceBoundary {
    system: SystemState,
    handoff: RelayHandoff,
    endpoint: EndpointCircuit,
    key: BoundaryKey,
    checkpoints: Vec<BoundaryCheckpoint>,
}

impl ReferenceBoundary {
    fn remote_rejection() -> Self {
        let key = BoundaryKey {
            raw: BoundaryTxId(1),
            witness: BoundaryWitnessId(1),
            source: BoundarySource::Remote(BoundaryPeerId(7)),
            request: BoundaryRequestId(1),
        };
        Self {
            system: SystemState::constructing(ProtocolLimits::small()),
            handoff: RelayHandoff::new(RelayLimits {
                records: 2,
                bytes: 16,
            }),
            endpoint: EndpointCircuit::Available,
            key,
            checkpoints: vec![BoundaryCheckpoint::Lifecycle(
                BoundaryLifecycleState::Constructing,
            )],
        }
    }

    fn assemble_and_run(&mut self) {
        let assembled = self.system.step(SystemEvent::Assemble {
            limits: ModelLimits::small(),
            view: ViewId(1),
            rules: RulesId(1),
            succeed: true,
        });
        debug_assert_eq!(assembled, SystemDisposition::Assembled);
        self.observe_lifecycle();
        let ready = self.system.step(SystemEvent::Ready);
        debug_assert_eq!(ready, SystemDisposition::Running);
        self.observe_lifecycle();
    }

    fn offer(&mut self) {
        let item = self.item();
        let source = match self.key.source {
            BoundarySource::Remote(peer) => RelaySource::Remote(PeerId(peer.0)),
            BoundarySource::Proposal => RelaySource::Proposal,
        };
        debug_assert_eq!(
            self.handoff.offer(item, source, PayloadCost::small().bytes),
            RelayDisposition::Offered(item)
        );
        self.checkpoints.push(BoundaryCheckpoint::Handoff {
            key: self.key,
            state: BoundaryHandoffState::CallerOwned,
        });
    }

    fn enqueue(&mut self) {
        let kind = self.controller_kind();
        debug_assert_eq!(
            self.system.step(SystemEvent::Enqueue {
                request: RequestId(self.key.request.0),
                kind,
                cost: PayloadCost::small(),
            }),
            SystemDisposition::Enqueued(RequestId(self.key.request.0))
        );
        let item = self.item();
        debug_assert_eq!(
            self.handoff
                .enqueue(item, RequestId(self.key.request.0), true),
            RelayDisposition::Enqueued(item)
        );
        self.checkpoints.push(BoundaryCheckpoint::Controller {
            key: self.key,
            state: BoundaryControllerState::Queued,
        });
        self.checkpoints.push(BoundaryCheckpoint::Handoff {
            key: self.key,
            state: BoundaryHandoffState::Queued,
        });
    }

    fn fail_enqueue(&mut self, failure: BoundaryEnqueueFailure) {
        match failure {
            BoundaryEnqueueFailure::Full => {
                for request in [RequestId(2), RequestId(3)] {
                    debug_assert_eq!(
                        self.system.step(SystemEvent::Enqueue {
                            request,
                            kind: RequestKind::Notification,
                            cost: PayloadCost::small(),
                        }),
                        SystemDisposition::Enqueued(request)
                    );
                }
            }
            BoundaryEnqueueFailure::Closed => {
                debug_assert_eq!(
                    self.system.step(SystemEvent::BeginDrain),
                    SystemDisposition::Draining
                );
                self.observe_lifecycle();
            }
        }
        let expected = match failure {
            BoundaryEnqueueFailure::Full => {
                SystemDisposition::QueueFull(RequestId(self.key.request.0))
            }
            BoundaryEnqueueFailure::Closed => {
                SystemDisposition::ChannelClosed(RequestId(self.key.request.0))
            }
        };
        debug_assert_eq!(
            self.system.step(SystemEvent::Enqueue {
                request: RequestId(self.key.request.0),
                kind: self.controller_kind(),
                cost: PayloadCost::small(),
            }),
            expected
        );
        let item = self.item();
        debug_assert_eq!(
            self.handoff
                .enqueue(item, RequestId(self.key.request.0), false),
            RelayDisposition::Released(item)
        );
        self.checkpoints.push(BoundaryCheckpoint::Controller {
            key: self.key,
            state: match failure {
                BoundaryEnqueueFailure::Full => BoundaryControllerState::Full,
                BoundaryEnqueueFailure::Closed => BoundaryControllerState::Closed,
            },
        });
        self.checkpoints.push(BoundaryCheckpoint::Handoff {
            key: self.key,
            state: BoundaryHandoffState::Released,
        });
        if failure == BoundaryEnqueueFailure::Closed {
            debug_assert_eq!(
                self.system.step(SystemEvent::FinishDrain),
                SystemDisposition::Stopped
            );
            self.observe_lifecycle();
        }
    }

    fn dispatch(&mut self) {
        debug_assert_eq!(
            self.system
                .step(SystemEvent::Dispatch(RequestId(self.key.request.0))),
            SystemDisposition::Dispatched(RequestId(self.key.request.0))
        );
        let item = self.item();
        debug_assert_eq!(
            self.handoff.dispatch(item, RequestId(self.key.request.0)),
            RelayDisposition::Dispatched(item)
        );
        self.checkpoints.push(BoundaryCheckpoint::Controller {
            key: self.key,
            state: BoundaryControllerState::HandlerOwned,
        });
        self.checkpoints.push(BoundaryCheckpoint::Handoff {
            key: self.key,
            state: BoundaryHandoffState::HandlerOwned,
        });
    }

    fn commit_rejection(&mut self) {
        let item = self.item();
        debug_assert_eq!(
            self.handoff
                .authority_accept(item, RequestId(self.key.request.0)),
            RelayDisposition::AuthorityAccepted(item)
        );
        self.checkpoints.push(BoundaryCheckpoint::Handoff {
            key: self.key,
            state: BoundaryHandoffState::AuthorityOwned,
        });
        let BoundarySource::Remote(peer) = self.key.source else {
            return;
        };
        let step = self.system.step(SystemEvent::Kernel {
            access: KernelAccess::Ordinary,
            command: KernelCommand::BanPeer {
                peer: PeerId(peer.0),
                observed_at: MonotonicTick(1),
            },
        });
        debug_assert!(matches!(
            step,
            SystemDisposition::Kernel(KernelStep::AuthorityCommit {
                disposition: KernelDisposition::PeerBanned { .. },
                ..
            })
        ));
        self.checkpoints
            .push(BoundaryCheckpoint::AuthorityRejected { key: self.key });
        self.checkpoints.push(BoundaryCheckpoint::Effect {
            key: self.key,
            state: BoundaryEffectState::Committed,
        });
    }

    fn finish_controller(&mut self) {
        let send_response = matches!(self.key.source, BoundarySource::Remote(_));
        let result = self.system.step(SystemEvent::Finish {
            request: RequestId(self.key.request.0),
            send_response,
        });
        let expected = if send_response {
            ResponseResult::Sent
        } else {
            ResponseResult::NotApplicable
        };
        debug_assert_eq!(
            result,
            SystemDisposition::Finished {
                request: RequestId(self.key.request.0),
                response: expected,
            }
        );
        self.checkpoints.push(BoundaryCheckpoint::Controller {
            key: self.key,
            state: if send_response {
                BoundaryControllerState::ResponseSent
            } else {
                BoundaryControllerState::NotificationFinished
            },
        });
    }

    fn claim_effect(&mut self) {
        let step = self.system.step(SystemEvent::Kernel {
            access: KernelAccess::Ordinary,
            command: KernelCommand::ClaimEffect,
        });
        debug_assert!(matches!(
            step,
            SystemDisposition::Kernel(KernelStep::NoAuthorityCommit(
                KernelDisposition::EffectClaimed(_)
            ))
        ));
        self.checkpoints.push(BoundaryCheckpoint::Effect {
            key: self.key,
            state: BoundaryEffectState::Claimed,
        });
    }

    fn publish_rejection(&mut self, settlement: BoundaryRelaySettlement) {
        match settlement {
            BoundaryRelaySettlement::ExactRelease | BoundaryRelaySettlement::ConservativeReset => {
                self.endpoint = self.endpoint.step(EndpointEvent::CallReturned);
                let item = self.item();
                debug_assert_eq!(
                    self.handoff.settle(item, RelayTerminal::Rejected),
                    RelayDisposition::Released(item)
                );
                self.checkpoints.push(BoundaryCheckpoint::Handoff {
                    key: self.key,
                    state: BoundaryHandoffState::Released,
                });
            }
            BoundaryRelaySettlement::CircuitDisposed => {
                self.endpoint = self.endpoint.step(EndpointEvent::Disable);
            }
        }
        self.checkpoints.push(BoundaryCheckpoint::Relay {
            key: self.key,
            settlement,
        });
    }

    fn settle_effect(&mut self) {
        let claim = self
            .system
            .authority
            .as_ref()
            .and_then(|authority| authority.linear.effect_claim);
        let Some(claim) = claim else {
            return;
        };
        let step = self.system.step(SystemEvent::Kernel {
            access: KernelAccess::Ordinary,
            command: KernelCommand::SettleEffect(claim),
        });
        debug_assert!(matches!(
            step,
            SystemDisposition::Kernel(KernelStep::AuthorityCommit {
                disposition: KernelDisposition::EffectSettled(_),
                ..
            })
        ));
        self.checkpoints.push(BoundaryCheckpoint::Effect {
            key: self.key,
            state: BoundaryEffectState::Settled,
        });
    }

    fn drain(&mut self) {
        debug_assert_eq!(
            self.system.step(SystemEvent::BeginDrain),
            SystemDisposition::Draining
        );
        self.observe_lifecycle();
        debug_assert_eq!(
            self.system.step(SystemEvent::FinishDrain),
            SystemDisposition::Stopped
        );
        self.observe_lifecycle();
    }

    fn observe_lifecycle(&mut self) {
        self.checkpoints
            .push(BoundaryCheckpoint::Lifecycle(match self.system.lifecycle {
                Lifecycle::Constructing => BoundaryLifecycleState::Constructing,
                Lifecycle::Initializing => BoundaryLifecycleState::Initializing,
                Lifecycle::Running => BoundaryLifecycleState::Running,
                Lifecycle::Draining => BoundaryLifecycleState::Draining,
                Lifecycle::Stopped => BoundaryLifecycleState::Stopped,
                Lifecycle::StartupFailed => BoundaryLifecycleState::StartupFailed,
            }));
    }

    fn item(&self) -> RelayItem {
        RelayItem {
            raw: TxId(self.key.raw.0),
            witness: WitnessId(self.key.witness.0),
        }
    }

    fn controller_kind(&self) -> RequestKind {
        match self.key.source {
            BoundarySource::Remote(_) => RequestKind::Ordinary { response: true },
            BoundarySource::Proposal => RequestKind::Notification,
        }
    }

    fn check(&self) {
        debug_assert_eq!(self.system.check_invariants(), Ok(()));
        debug_assert_eq!(self.handoff.check_invariants(), Ok(()));
        let expected_location = self
            .handoff
            .records
            .get(&TxId(self.key.raw.0))
            .map(|record| record.location);
        debug_assert!(matches!(
            expected_location,
            None | Some(
                RelayLocation::CallerOwned
                    | RelayLocation::Queued(_)
                    | RelayLocation::HandlerOwned(_)
                    | RelayLocation::AuthorityOwned
                    | RelayLocation::SettledKnown
            )
        ));
        if let Some(record) = self
            .system
            .protocol
            .requests
            .get(&RequestId(self.key.request.0))
        {
            debug_assert!(matches!(
                record.payload,
                PayloadLocation::Queued | PayloadLocation::HandlerOwned
            ));
            debug_assert!(matches!(
                record.response,
                None | Some(ResponseEndpoint::Attached | ResponseEndpoint::Abandoned)
            ));
        }
    }
}

pub(crate) fn reference_remote_rejection_boundary_trace() -> Vec<BoundaryCheckpoint> {
    reference_remote_rejection_boundary_trace_with_relay(BoundaryRelaySettlement::ConservativeReset)
}

pub(crate) fn reference_remote_rejection_boundary_trace_with_relay(
    settlement: BoundaryRelaySettlement,
) -> Vec<BoundaryCheckpoint> {
    let mut boundary = ReferenceBoundary::remote_rejection();
    boundary.assemble_and_run();
    boundary.offer();
    boundary.enqueue();
    boundary.dispatch();
    boundary.commit_rejection();
    boundary.finish_controller();
    boundary.claim_effect();
    boundary.publish_rejection(settlement);
    boundary.settle_effect();
    boundary.drain();
    boundary.check();
    boundary.checkpoints
}

pub(crate) fn reference_controller_success_boundary_trace(
    source: BoundarySource,
) -> Vec<BoundaryCheckpoint> {
    let mut boundary = ReferenceBoundary::remote_rejection();
    boundary.key.source = source;
    boundary.assemble_and_run();
    boundary.offer();
    boundary.enqueue();
    boundary.dispatch();
    boundary.finish_controller();
    boundary.check();
    boundary.checkpoints
}

pub(crate) fn reference_failed_enqueue_boundary_trace(
    source: BoundarySource,
    failure: BoundaryEnqueueFailure,
) -> Vec<BoundaryCheckpoint> {
    let mut boundary = ReferenceBoundary::remote_rejection();
    boundary.key.source = source;
    boundary.assemble_and_run();
    boundary.offer();
    boundary.fail_enqueue(failure);
    boundary.check();
    boundary.checkpoints
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boundary_trace_composes_controller_authority_effect_and_lifecycle_cuts() {
        let trace = reference_remote_rejection_boundary_trace();
        assert_eq!(
            trace.first(),
            Some(&BoundaryCheckpoint::Lifecycle(
                BoundaryLifecycleState::Constructing
            ))
        );
        assert!(trace.contains(&BoundaryCheckpoint::Lifecycle(
            BoundaryLifecycleState::Running
        )));
        assert!(trace.contains(&BoundaryCheckpoint::Effect {
            key: BoundaryKey {
                raw: BoundaryTxId(1),
                witness: BoundaryWitnessId(1),
                source: BoundarySource::Remote(BoundaryPeerId(7)),
                request: BoundaryRequestId(1),
            },
            state: BoundaryEffectState::Claimed,
        }));
        assert_eq!(
            trace.last(),
            Some(&BoundaryCheckpoint::Lifecycle(
                BoundaryLifecycleState::Stopped
            ))
        );
    }

    #[test]
    fn boundary_enqueue_failure_releases_remote_and_proposal_handoffs_exactly_once() {
        for source in [
            BoundarySource::Remote(BoundaryPeerId(7)),
            BoundarySource::Proposal,
        ] {
            for failure in [BoundaryEnqueueFailure::Full, BoundaryEnqueueFailure::Closed] {
                let trace = reference_failed_enqueue_boundary_trace(source, failure);
                let key = BoundaryKey {
                    raw: BoundaryTxId(1),
                    witness: BoundaryWitnessId(1),
                    source,
                    request: BoundaryRequestId(1),
                };
                assert!(trace.contains(&BoundaryCheckpoint::Controller {
                    key,
                    state: match failure {
                        BoundaryEnqueueFailure::Full => BoundaryControllerState::Full,
                        BoundaryEnqueueFailure::Closed => BoundaryControllerState::Closed,
                    },
                }));
                assert_eq!(
                    trace
                        .iter()
                        .filter(|checkpoint| {
                            **checkpoint
                                == BoundaryCheckpoint::Handoff {
                                    key,
                                    state: BoundaryHandoffState::Released,
                                }
                        })
                        .count(),
                    1
                );
            }
        }
    }

    #[test]
    fn boundary_effect_settlement_is_independent_of_external_relay_availability() {
        for settlement in [
            BoundaryRelaySettlement::ExactRelease,
            BoundaryRelaySettlement::ConservativeReset,
            BoundaryRelaySettlement::CircuitDisposed,
        ] {
            let trace = reference_remote_rejection_boundary_trace_with_relay(settlement);
            assert!(trace.contains(&BoundaryCheckpoint::Relay {
                key: BoundaryKey {
                    raw: BoundaryTxId(1),
                    witness: BoundaryWitnessId(1),
                    source: BoundarySource::Remote(BoundaryPeerId(7)),
                    request: BoundaryRequestId(1),
                },
                settlement,
            }));
            assert_eq!(
                trace.last(),
                Some(&BoundaryCheckpoint::Lifecycle(
                    BoundaryLifecycleState::Stopped
                ))
            );
        }
    }
}
