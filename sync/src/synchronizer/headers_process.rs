use crate::synchronizer::Synchronizer;
use crate::types::{ActiveChain, SyncShared};
use crate::{Status, StatusCode};
use ckb_constant::sync::MAX_HEADERS_LEN;
use ckb_error::Error;
use ckb_logger::{debug, log_enabled, warn, Level};
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_shared::block_status::BlockStatus;
use ckb_traits::HeaderFieldsProvider;
use ckb_types::{core, packed, prelude::*};
use ckb_verification::{HeaderError, HeaderVerifier};
use ckb_verification_traits::Verifier;

pub struct HeadersProcess<'a> {
    message: packed::SendHeadersReader<'a>,
    synchronizer: &'a Synchronizer,
    peer: PeerIndex,
    nc: &'a dyn CKBProtocolContext,
    active_chain: ActiveChain,
}

impl<'a> HeadersProcess<'a> {
    pub fn new(
        message: packed::SendHeadersReader<'a>,
        synchronizer: &'a Synchronizer,
        peer: PeerIndex,
        nc: &'a dyn CKBProtocolContext,
    ) -> Self {
        let active_chain = synchronizer.shared.active_chain();
        HeadersProcess {
            message,
            nc,
            synchronizer,
            peer,
            active_chain,
        }
    }

    fn is_continuous(&self, headers: &[core::HeaderView]) -> bool {
        for window in headers.windows(2) {
            if let [parent, header] = &window {
                if header.data().raw().parent_hash() != parent.hash() {
                    debug!(
                        "header.parent_hash {} parent.hash {}",
                        header.parent_hash(),
                        parent.hash()
                    );
                    return false;
                }
            }
        }
        true
    }

    fn is_parent_exists(&self, first_header: &core:HeaderView) -> bool {
        let shared: &SyncShared = self.synchronizer.shared();
        shared.get_header_fields(first_header.parent_hash).is_some()
    }


    pub fn accept_first(&self, first: &core::HeaderView) -> ValidationResult {
        let shared: &SyncShared = self.synchronizer.shared();
        let verifier = HeaderVerifier::new(shared, shared.consensus());
        let acceptor = HeaderAcceptor::new(first, self.peer, verifier, self.active_chain.clone());
        acceptor.accept()
    }

    fn debug(&self) {
        if log_enabled!(Level::Debug) {
            // Regain the updated best known
            let shared_best_known = self.synchronizer.shared.state().shared_best_header();
            let peer_best_known = self.synchronizer.peers().get_best_known_header(self.peer);
            debug!(
                "chain: num={}, diff={:#x};",
                self.active_chain.tip_number(),
                self.active_chain.total_difficulty()
            );
            debug!(
                "shared best_known_header: num={}, diff={:#x}, hash={};",
                shared_best_known.number(),
                shared_best_known.total_difficulty(),
                shared_best_known.hash(),
            );
            if let Some(header) = peer_best_known {
                debug!(
                    "peer's best_known_header: peer: {}, num={}; diff={:#x}, hash={};",
                    self.peer,
                    header.number(),
                    header.total_difficulty(),
                    header.hash()
                );
            } else {
                debug!("state: null;");
            }
            debug!("peer: {}", self.peer);
        }
    }

    pub fn execute(self) -> Status {
        debug!("HeadersProcess begins");
        let shared: &SyncShared = self.synchronizer.shared();
        let consensus = shared.consensus();
        let headers = self
            .message
            .headers()
            .to_entity()
            .into_iter()
            .map(packed::Header::into_view)
            .collect::<Vec<_>>();

        if headers.len() > MAX_HEADERS_LEN {
            warn!("HeadersProcess is oversized");
            return StatusCode::HeadersIsInvalid.with_context("oversize");
        }

        if headers.is_empty() {
            // Empty means that the other peer's tip may be consistent with our own best known,
            // but empty cannot 100% confirm this, so it does not set the other peer's best header
            // to the shared best known.
            // This action means that if the newly connected node has not been sync with headers,
            // it cannot be used as a synchronization node.
            debug!("HeadersProcess is_empty (synchronized)");
            if let Some(mut state) = self.synchronizer.peers().state.get_mut(&self.peer) {
                self.synchronizer
                    .shared()
                    .state()
                    .tip_synced(state.value_mut());
            }
            return Status::ok();
        }

        if !self.is_continuous(&headers) {
            warn!("HeadersProcess is not continuous");
            return StatusCode::HeadersIsInvalid.with_context("not continuous");
        }

        if !self.is_parent_exists(&headers[0]) {
            // put the headers into a memory cache
            self.synchronizer.header_cache.insert(headers[0].parent_hash, headers);
            // verify them later
            return Status::ok();
        }

        let result = self.accept_first(&headers[0]);
        match result.state {
            ValidationState::Invalid => {
                debug!(
                    "HeadersProcess accept_first result is invalid, error = {:?}, first header = {:?}",
                    result.error, headers[0]
                );
                return StatusCode::HeadersIsInvalid
                    .with_context(format!("accept first header {:?}", headers[0]));
            }
            ValidationState::TemporaryInvalid => {
                debug!(
                    "HeadersProcess accept_first result is temporary invalid, first header = {:?}",
                    headers[0]
                );
                return Status::ok();
            }
            ValidationState::Valid => {
                // Valid, do nothing
            }
        };

        for header in headers.iter().skip(1) {
            let verifier = HeaderVerifier::new(shared, consensus);
            let acceptor =
                HeaderAcceptor::new(header, self.peer, verifier, self.active_chain.clone());
            let result = acceptor.accept();
            match result.state {
                ValidationState::Invalid => {
                    debug!(
                        "HeadersProcess accept result is invalid, error = {:?}, header = {:?}",
                        result.error, headers,
                    );
                    return StatusCode::HeadersIsInvalid
                        .with_context(format!("accept header {header:?}"));
                }
                ValidationState::TemporaryInvalid => {
                    debug!(
                        "HeadersProcess accept result is temporarily invalid, header = {:?}",
                        header
                    );
                    return Status::ok();
                }
                ValidationState::Valid => {
                    // Valid, do nothing
                }
            };
        }

        self.debug();

        if headers.len() == MAX_HEADERS_LEN {
            let start = headers.last().expect("empty checked").into();
            self.active_chain
                .send_getheaders_to_peer(self.nc, self.peer, start);
        } else if let Some(mut state) = self.synchronizer.peers().state.get_mut(&self.peer) {
            self.synchronizer
                .shared()
                .state()
                .tip_synced(state.value_mut());
        }

        // If we're in IBD, we want outbound peers that will serve us a useful
        // chain. Disconnect peers that are on chains with insufficient work.
        let peer_flags = self
            .synchronizer
            .peers()
            .get_flag(self.peer)
            .unwrap_or_default();
        if self.active_chain.is_initial_block_download()
            && headers.len() != MAX_HEADERS_LEN
            && (!peer_flags.is_protect && !peer_flags.is_whitelist && peer_flags.is_outbound)
        {
            debug!("Disconnect an unprotected outbound peer ({})", self.peer);
            if let Err(err) = self
                .nc
                .disconnect(self.peer, "useless outbound peer in IBD")
            {
                return StatusCode::Network.with_context(format!("Disconnect error: {err:?}"));
            }
        }

        {
            // these headers verify success
            // may the headers's tail header_hash exist in headers_cahce?
            if let Some(headers) = self.synchronizer.headers_cache.get(headers.last().expect("last header must exist").hash){
                HeadersProcess::new().execute();
            }
        }

        Status::ok()
    }
}

pub struct HeaderAcceptor<'a, DL: HeaderFieldsProvider> {
    header: &'a core::HeaderView,
    active_chain: ActiveChain,
    peer: PeerIndex,
    verifier: HeaderVerifier<'a, DL>,
}

impl<'a, DL: HeaderFieldsProvider> HeaderAcceptor<'a, DL> {
    pub fn new(
        header: &'a core::HeaderView,
        peer: PeerIndex,
        verifier: HeaderVerifier<'a, DL>,
        active_chain: ActiveChain,
    ) -> Self {
        HeaderAcceptor {
            header,
            peer,
            verifier,
            active_chain,
        }
    }

    pub fn prev_block_check(&self, state: &mut ValidationResult) -> Result<(), ()> {
        if self.active_chain.contains_block_status(
            &self.header.data().raw().parent_hash(),
            BlockStatus::BLOCK_INVALID,
        ) {
            state.invalid(Some(ValidationError::InvalidParent));
            return Err(());
        }
        Ok(())
    }

    pub fn non_contextual_check(&self, state: &mut ValidationResult) -> Result<(), bool> {
        self.verifier.verify(self.header).map_err(|error| {
            debug!(
                "HeadersProcess accepted {:?} error {:?}",
                self.header.number(),
                error
            );
            // HeaderVerifier return HeaderError or UnknownParentError
            if let Some(header_error) = error.downcast_ref::<HeaderError>() {
                if header_error.is_too_new() {
                    state.temporary_invalid(Some(ValidationError::Verify(error)));
                    false
                } else {
                    state.invalid(Some(ValidationError::Verify(error)));
                    true
                }
            } else {
                state.invalid(Some(ValidationError::Verify(error)));
                true
            }
        })
    }

    pub fn version_check(&self, state: &mut ValidationResult) -> Result<(), ()> {
        if self.header.version() != 0 {
            state.invalid(Some(ValidationError::Version));
            Err(())
        } else {
            Ok(())
        }
    }

    pub fn accept(&self) -> ValidationResult {
        let mut result = ValidationResult::default();
        let sync_shared = self.active_chain.sync_shared();
        let state = self.active_chain.state();
        let shared = sync_shared.shared();

        // FIXME If status == BLOCK_INVALID then return early. But which error
        // type should we return?
        let status = self.active_chain.get_block_status(&self.header.hash());
        if status.contains(BlockStatus::HEADER_VALID) {
            let header_index = sync_shared
                .get_header_index_view(
                    &self.header.hash(),
                    status.contains(BlockStatus::BLOCK_STORED),
                )
                .unwrap_or_else(|| {
                    panic!(
                        "header {}-{} with HEADER_VALID should exist",
                        self.header.number(),
                        self.header.hash()
                    )
                })
                .as_header_index();
            state
                .peers()
                .may_set_best_known_header(self.peer, header_index);
            return result;
        }

        if self.prev_block_check(&mut result).is_err() {
            debug!(
                "HeadersProcess rejected invalid-parent header: {} {}",
                self.header.number(),
                self.header.hash(),
            );
            shared.insert_block_status(self.header.hash(), BlockStatus::BLOCK_INVALID);
            return result;
        }

        if let Some(is_invalid) = self.non_contextual_check(&mut result).err() {
            debug!(
                "HeadersProcess rejected non-contextual header: {} {}",
                self.header.number(),
                self.header.hash(),
            );
            if is_invalid {
                shared.insert_block_status(self.header.hash(), BlockStatus::BLOCK_INVALID);
            }
            return result;
        }

        if self.version_check(&mut result).is_err() {
            debug!(
                "HeadersProcess rejected invalid-version header: {} {}",
                self.header.number(),
                self.header.hash(),
            );
            shared.insert_block_status(self.header.hash(), BlockStatus::BLOCK_INVALID);
            return result;
        }

        sync_shared.insert_valid_header(self.peer, self.header);
        result
    }
}

#[derive(Default, Debug, Copy, Clone, PartialEq, Eq)]
pub enum ValidationState {
    #[default]
    Valid,
    TemporaryInvalid,
    Invalid,
}

#[allow(dead_code)]
#[derive(Debug)]
pub enum ValidationError {
    Verify(Error),
    Version,
    InvalidParent,
}

#[derive(Debug, Default)]
pub struct ValidationResult {
    pub error: Option<ValidationError>,
    pub state: ValidationState,
}

impl ValidationResult {
    pub fn invalid(&mut self, error: Option<ValidationError>) {
        self.error = error;
        self.state = ValidationState::Invalid;
    }

    pub fn temporary_invalid(&mut self, error: Option<ValidationError>) {
        self.error = error;
        self.state = ValidationState::TemporaryInvalid;
    }
}
