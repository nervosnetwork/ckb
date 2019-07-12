use crate::block_status::BlockStatus;
use crate::synchronizer::Synchronizer;
use crate::MAX_HEADERS_LEN;
use ckb_core::extras::EpochExt;
use ckb_core::header::Header;
use ckb_logger::{debug, log_enabled, warn, Level};
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_protocol::{cast, FlatbuffersVectorIterator, Headers};
use ckb_store::ChainStore;
use ckb_traits::BlockMedianTimeContext;
use ckb_verification::{Error as VerifyError, HeaderResolver, HeaderVerifier, Verifier};
use failure::Error as FailureError;
use numext_fixed_hash::H256;
use std::convert::TryInto;
use std::sync::Arc;

pub struct HeadersProcess<'a, CS: ChainStore + 'a> {
    message: &'a Headers<'a>,
    synchronizer: &'a Synchronizer<CS>,
    peer: PeerIndex,
    nc: &'a CKBProtocolContext,
}

pub struct VerifierResolver<'a, CS: ChainStore + 'a> {
    synchronizer: &'a Synchronizer<CS>,
    header: &'a Header,
    parent: Option<&'a Header>,
    epoch: Option<EpochExt>,
}

impl<'a, CS: ChainStore + 'a> VerifierResolver<'a, CS> {
    pub fn new(
        parent: Option<&'a Header>,
        header: &'a Header,
        synchronizer: &'a Synchronizer<CS>,
    ) -> Self {
        let epoch = parent
            .and_then(|parent| {
                synchronizer
                    .shared
                    .get_epoch_ext(&parent.hash())
                    .map(|ext| (parent, ext))
            })
            .map(|(parent, last_epoch)| {
                synchronizer
                    .shared
                    .next_epoch_ext(&last_epoch, parent)
                    .unwrap_or(last_epoch)
            });

        VerifierResolver {
            parent,
            header,
            synchronizer,
            epoch,
        }
    }
}

impl<'a, CS: ChainStore> ::std::clone::Clone for VerifierResolver<'a, CS> {
    fn clone(&self) -> Self {
        VerifierResolver {
            parent: self.parent,
            header: self.header,
            synchronizer: self.synchronizer,
            epoch: self.epoch.clone(),
        }
    }
}

impl<'a, CS: ChainStore + 'a> BlockMedianTimeContext for VerifierResolver<'a, CS> {
    fn median_block_count(&self) -> u64 {
        self.synchronizer
            .shared
            .consensus()
            .median_time_block_count() as u64
    }

    fn timestamp_and_parent(&self, block_hash: &H256) -> (u64, H256) {
        let header = self
            .synchronizer
            .shared
            .get_header(&block_hash)
            .expect("[VerifierResolver] blocks used for median time exist");
        (header.timestamp(), header.parent_hash().to_owned())
    }
}

impl<'a, CS: ChainStore> HeaderResolver for VerifierResolver<'a, CS> {
    fn header(&self) -> &Header {
        self.header
    }

    fn parent(&self) -> Option<&Header> {
        self.parent
    }

    fn epoch(&self) -> Option<&EpochExt> {
        self.epoch.as_ref()
    }
}

impl<'a, CS> HeadersProcess<'a, CS>
where
    CS: ChainStore + 'a,
{
    pub fn new(
        message: &'a Headers,
        synchronizer: &'a Synchronizer<CS>,
        peer: PeerIndex,
        nc: &'a CKBProtocolContext,
    ) -> Self {
        HeadersProcess {
            message,
            nc,
            synchronizer,
            peer,
        }
    }

    fn is_continuous(&self, headers: &[Header]) -> bool {
        for window in headers.windows(2) {
            if let [parent, header] = &window {
                if header.parent_hash() != parent.hash() {
                    debug!(
                        "header.parent_hash {:x} parent.hash {:x}",
                        header.parent_hash(),
                        parent.hash()
                    );
                    return false;
                }
            }
        }
        true
    }

    fn received_new_header(&self, headers: &[Header]) -> bool {
        let last = headers.last().expect("empty checked");
        self.synchronizer.shared().unknown_block_status(last.hash())
    }

    pub fn accept_first(&self, first: &Header) -> ValidationResult {
        let parent = self.synchronizer.shared.get_header(&first.parent_hash());
        let resolver = VerifierResolver::new(parent.as_ref(), &first, &self.synchronizer);
        let verifier = HeaderVerifier::new(
            resolver.clone(),
            Arc::clone(&self.synchronizer.shared.consensus().pow_engine()),
        );
        let acceptor =
            HeaderAcceptor::new(first, self.peer, &self.synchronizer, resolver, verifier);
        acceptor.accept()
    }

    pub fn execute(self) -> Result<(), FailureError> {
        debug!("HeadersProcess begin");

        let headers = cast!(self.message.headers())?;

        if headers.len() > MAX_HEADERS_LEN {
            self.synchronizer.shared().misbehavior(self.peer, 20);
            warn!("HeadersProcess is_oversize");
            return Ok(());
        }

        if headers.len() == 0 {
            // Reset headers sync timeout
            self.synchronizer
                .peers()
                .state
                .write()
                .get_mut(&self.peer)
                .expect("Peer must exists")
                .headers_sync_timeout = None;
            debug!("HeadersProcess is_empty (synchronized)");
            return Ok(());
        }

        let headers = FlatbuffersVectorIterator::new(headers)
            .map(TryInto::try_into)
            .collect::<Result<Vec<Header>, FailureError>>()?;

        if !self.is_continuous(&headers) {
            self.synchronizer.shared().misbehavior(self.peer, 20);
            debug!("HeadersProcess is not continuous");
            return Ok(());
        }

        let result = self.accept_first(&headers[0]);
        if !result.is_valid() {
            if result.misbehavior > 0 {
                self.synchronizer
                    .shared()
                    .misbehavior(self.peer, result.misbehavior);
            }
            debug!(
                "HeadersProcess accept_first is_valid {:?} headers = {:?}",
                result, headers[0]
            );
            return Ok(());
        }

        for window in headers.windows(2) {
            if let [parent, header] = &window {
                let resolver = VerifierResolver::new(Some(&parent), &header, &self.synchronizer);
                let verifier = HeaderVerifier::new(
                    resolver.clone(),
                    Arc::clone(&self.synchronizer.shared.consensus().pow_engine()),
                );
                let acceptor =
                    HeaderAcceptor::new(&header, self.peer, &self.synchronizer, resolver, verifier);
                let result = acceptor.accept();

                if !result.is_valid() {
                    if result.misbehavior > 0 {
                        self.synchronizer
                            .shared()
                            .misbehavior(self.peer, result.misbehavior);
                    }
                    debug!("HeadersProcess accept is invalid {:?}", result);
                    return Ok(());
                }
            }
        }

        if log_enabled!(Level::Debug) {
            // Regain the updated best known
            let chain_state = self.synchronizer.shared.lock_chain_state();
            let shared_best_known = self.synchronizer.shared.shared_best_header();
            let peer_best_known = self.synchronizer.peers().get_best_known_header(self.peer);
            debug!(
                "chain: num={}, diff={:#x};",
                chain_state.tip_number(),
                chain_state.total_difficulty()
            );
            debug!(
                "shared best_known_header: num={}, diff={:#x}, hash={:#x};",
                shared_best_known.number(),
                shared_best_known.total_difficulty(),
                shared_best_known.hash(),
            );
            if let Some(header) = peer_best_known {
                debug!(
                    "peer's best_known_header: peer: {}, num={}; diff={:#x}, hash={:#x};",
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

        if self.received_new_header(&headers) {
            // update peer last_block_announcement
        }

        // TODO: optimize: if last is an ancestor of BestKnownHeader, continue from there instead.
        if headers.len() == MAX_HEADERS_LEN {
            let start = headers.last().expect("empty checked");
            self.synchronizer
                .shared
                .send_getheaders_to_peer(self.nc, self.peer, start);
        }

        // If we're in IBD, we want outbound peers that will serve us a useful
        // chain. Disconnect peers that are on chains with insufficient work.
        let (is_outbound, is_protected) = self
            .synchronizer
            .peers()
            .state
            .read()
            .get(&self.peer)
            .map(|state| (state.is_outbound, state.chain_sync.protect))
            .unwrap_or((false, false));
        if self.synchronizer.shared.is_initial_block_download()
            && headers.len() != MAX_HEADERS_LEN
            && (is_outbound && !is_protected)
        {
            debug!("Disconnect peer({}) is unprotected outbound", self.peer);
            if let Err(err) = self.nc.disconnect(self.peer) {
                debug!("synchronizer disconnect error: {:?}", err);
            }
        }

        Ok(())
    }
}

#[derive(Clone)]
pub struct HeaderAcceptor<'a, V: Verifier, CS: ChainStore + 'a> {
    header: &'a Header,
    synchronizer: &'a Synchronizer<CS>,
    peer: PeerIndex,
    resolver: V::Target,
    verifier: V,
}

impl<'a, V, CS> HeaderAcceptor<'a, V, CS>
where
    V: Verifier<Target = VerifierResolver<'a, CS>>,
    CS: ChainStore + 'a,
{
    pub fn new(
        header: &'a Header,
        peer: PeerIndex,
        synchronizer: &'a Synchronizer<CS>,
        resolver: VerifierResolver<'a, CS>,
        verifier: V,
    ) -> Self {
        HeaderAcceptor {
            header,
            peer,
            resolver,
            verifier,
            synchronizer,
        }
    }

    //FIXME: status flag
    pub fn duplicate_check(&self, _state: &mut ValidationResult) -> Result<(), ()> {
        // let status = self.synchronizer.get_block_status(&self.header.hash());
        // if status != BlockStatus::UNKNOWN {
        //     if (status & BlockStatus::FAILED_MASK) == status {
        //         state.invalid(Some(ValidationError::FailedMask));
        //     }
        //     if (status & BlockStatus::FAILED_MASK) == status {}
        //     return Err(());
        // }
        Ok(())
    }

    pub fn prev_block_check(&self, state: &mut ValidationResult) -> Result<(), ()> {
        if self
            .synchronizer
            .shared()
            .contains_block_status(self.header.parent_hash(), BlockStatus::BLOCK_INVALID)
        {
            state.dos(Some(ValidationError::InvalidParent), 100);
            return Err(());
        }
        Ok(())
    }

    pub fn non_contextual_check(&self, state: &mut ValidationResult) -> Result<(), ()> {
        self.verifier
            .verify(&self.resolver)
            .map_err(|error| match error {
                VerifyError::Pow(e) => {
                    debug!(
                        "HeadersProcess accept {:?} pow error {:?}",
                        self.header.number(),
                        e
                    );
                    state.dos(Some(ValidationError::Verify(VerifyError::Pow(e))), 100);
                }
                VerifyError::Epoch(e) => {
                    debug!(
                        "HeadersProcess accept {:?} epoch error {:?}",
                        self.header.number(),
                        e
                    );
                    state.dos(Some(ValidationError::Verify(VerifyError::Epoch(e))), 50);
                }
                error => {
                    debug!(
                        "HeadersProcess accept {:?} {:?}",
                        self.header.number(),
                        error
                    );
                    state.invalid(Some(ValidationError::Verify(error)));
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

        // FIXME If status == BLOCK_INVALID then return early. But which error
        // type should we return?
        if self
            .synchronizer
            .shared()
            .contains_block_status(self.header.hash(), BlockStatus::HEADER_VALID)
        {
            let header_view = self
                .synchronizer
                .shared()
                .get_header_view(self.header.hash())
                .expect("header with HEADER_VALID should exist");
            self.synchronizer
                .peers()
                .new_header_received(self.peer, &header_view);
            return result;
        }

        if self.duplicate_check(&mut result).is_err() {
            debug!(
                "HeadersProcess reject duplicate header: {} {:#x}",
                self.header.number(),
                self.header.hash()
            );
            return result;
        }

        if self.prev_block_check(&mut result).is_err() {
            debug!(
                "HeadersProcess reject invalid-parent header: {} {:#x}",
                self.header.number(),
                self.header.hash(),
            );
            self.synchronizer
                .shared()
                .insert_block_status(self.header.hash().to_owned(), BlockStatus::BLOCK_INVALID);
            return result;
        }

        if self.non_contextual_check(&mut result).is_err() {
            debug!(
                "HeadersProcess reject non-contextual header: {} {:#x}",
                self.header.number(),
                self.header.hash(),
            );
            self.synchronizer
                .shared()
                .insert_block_status(self.header.hash().to_owned(), BlockStatus::BLOCK_INVALID);
            return result;
        }

        if self.version_check(&mut result).is_err() {
            debug!(
                "HeadersProcess reject invalid-version header {} {:#x}",
                self.header.number(),
                self.header.hash(),
            );
            self.synchronizer
                .shared()
                .insert_block_status(self.header.hash().to_owned(), BlockStatus::BLOCK_INVALID);
            return result;
        }

        let epoch = self.resolver.epoch().expect("epoch verified").clone();
        self.synchronizer
            .shared()
            .insert_valid_header(self.peer, &self.header, epoch);
        result
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum ValidationState {
    VALID,
    INVALID,
}

impl Default for ValidationState {
    fn default() -> Self {
        ValidationState::VALID
    }
}

#[derive(Debug)]
pub enum ValidationError {
    Verify(VerifyError),
    Version,
    InvalidParent,
}

#[derive(Debug, Default)]
pub struct ValidationResult {
    pub error: Option<ValidationError>,
    pub misbehavior: u32,
    pub state: ValidationState,
}

impl ValidationResult {
    pub fn invalid(&mut self, error: Option<ValidationError>) {
        self.dos(error, 0);
    }

    pub fn dos(&mut self, error: Option<ValidationError>, misbehavior: u32) {
        self.error = error;
        self.misbehavior += misbehavior;
        self.state = ValidationState::INVALID;
    }

    pub fn is_valid(&self) -> bool {
        self.state == ValidationState::VALID
    }
}
