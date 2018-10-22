use bigint::U256;
use ckb_protocol::{FlatbuffersVectorIterator, Headers};
use ckb_shared::index::ChainIndex;
use ckb_shared::shared::ChainProvider;
use ckb_verification::{Error as VerifyError, HeaderResolver, HeaderVerifier, Verifier};
use core::header::Header;
use log;
use network::{CKBProtocolContext, PeerIndex};
use std::sync::Arc;
use synchronizer::{BlockStatus, Synchronizer};
use MAX_HEADERS_LEN;

pub struct HeadersProcess<'a, CI: ChainIndex + 'a> {
    message: &'a Headers<'a>,
    synchronizer: &'a Synchronizer<CI>,
    peer: PeerIndex,
    nc: &'a CKBProtocolContext,
}

pub struct VerifierResolver<'a, CP: 'a> {
    provider: &'a CP,
    header: &'a Header,
    parent: Option<&'a Header>,
}

impl<'a, CP: ChainProvider> VerifierResolver<'a, CP> {
    pub fn new(parent: Option<&'a Header>, header: &'a Header, provider: &'a CP) -> Self {
        VerifierResolver {
            parent,
            header,
            provider,
        }
    }
}

impl<'a, CP: ChainProvider> HeaderResolver for VerifierResolver<'a, CP> {
    fn header(&self) -> &Header {
        self.header
    }

    fn parent(&self) -> Option<&Header> {
        self.parent
    }

    fn calculate_difficulty(&self) -> Option<U256> {
        self.parent()
            .and_then(|parent| self.provider.calculate_difficulty(parent))
    }
}

impl<'a, CI> HeadersProcess<'a, CI>
where
    CI: ChainIndex + 'a,
{
    pub fn new(
        message: &'a Headers,
        synchronizer: &'a Synchronizer<CI>,
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

    fn is_empty(&self) -> bool {
        self.message.headers().unwrap().len() == 0
    }

    fn is_oversize(&self) -> bool {
        self.message.headers().unwrap().len() > MAX_HEADERS_LEN
    }

    fn is_continuous(&self, headers: &[Header]) -> bool {
        for window in headers.windows(2) {
            if let [parent, header] = &window {
                if header.parent_hash() != parent.hash() {
                    return false;
                }
            }
        }
        true
    }

    fn received_new_header(&self, headers: &[Header]) -> bool {
        let last = headers.last().expect("empty checked");
        self.synchronizer.get_block_status(&last.hash()) == BlockStatus::UNKNOWN
    }

    pub fn accept_first(&self, first: &Header) -> ValidationResult {
        let parent = self.synchronizer.get_header(&first.parent_hash());
        let resolver = VerifierResolver::new(parent.as_ref(), &first, &self.synchronizer.shared);
        let verifier = HeaderVerifier::new(Arc::clone(&self.synchronizer.pow));
        let acceptor =
            HeaderAcceptor::new(first, self.peer, &self.synchronizer, resolver, verifier);
        acceptor.accept()
    }

    pub fn execute(self) {
        debug!(target: "sync", "HeadersProcess begin");

        if self.is_oversize() {
            self.synchronizer.peers.misbehavior(self.peer, 20);
            debug!(target: "sync", "HeadersProcess is_oversize");
            return ();
        }

        if self.is_empty() {
            debug!(target: "sync", "HeadersProcess is_empty");
            return ();
        }

        let headers = FlatbuffersVectorIterator::new(self.message.headers().unwrap())
            .map(Into::into)
            .collect::<Vec<Header>>();

        if !self.is_continuous(&headers) {
            self.synchronizer.peers.misbehavior(self.peer, 20);
            debug!(target: "sync", "HeadersProcess is not continuous");
            return ();
        }

        let result = self.accept_first(&headers[0]);
        if !result.is_valid() {
            if result.misbehavior > 0 {
                self.synchronizer
                    .peers
                    .misbehavior(self.peer, result.misbehavior);
            }
            debug!(target: "sync", "\n\nHeadersProcess accept_first is_valid {:?} headers = {:#?}\n\n", result, headers[0]);
            return ();
        }

        for window in headers.windows(2) {
            if let [parent, header] = &window {
                let resolver =
                    VerifierResolver::new(Some(&parent), &header, &self.synchronizer.shared);
                let verifier = HeaderVerifier::new(Arc::clone(&self.synchronizer.pow));
                let acceptor =
                    HeaderAcceptor::new(&header, self.peer, &self.synchronizer, resolver, verifier);
                let result = acceptor.accept();

                if !result.is_valid() {
                    if result.misbehavior > 0 {
                        self.synchronizer
                            .peers
                            .misbehavior(self.peer, result.misbehavior);
                    }
                    debug!(target: "sync", "HeadersProcess accept is invalid {:?}", result);
                    return ();
                }
            }
        }

        if log_enabled!(target: "sync", log::Level::Debug) {
            let own = { self.synchronizer.best_known_header.read().clone() };
            let chain_tip = self.synchronizer.shared.tip_header().read();
            let peer_state = self.synchronizer.peers.best_known_header(self.peer);
            debug!(
                target: "sync",
                concat!(
                    "\nchain total_difficulty = {}; number={}\n",
                    "number={}; best_known_header = {}; total_difficulty = {};\n",
                    "number={:?}; best_known_header = {:?}; total_difficulty = {:?}\n",
                ),
                chain_tip.total_difficulty(),
                chain_tip.number(),
                own.number(),
                own.hash(),
                own.total_difficulty(),
                peer_state.as_ref().map(|state| state.number()),
                peer_state.as_ref().map(|state| state.hash()),
                peer_state.as_ref().map(|state| state.total_difficulty()),
            );
        }

        if self.received_new_header(&headers) {
            // update peer last_block_announcement
        }

        // If we're in IBD, we want outbound peers that will serve us a useful
        // chain. Disconnect peers that are on chains with insufficient work.
        if self.synchronizer.is_initial_block_download() && headers.len() != MAX_HEADERS_LEN {}

        // TODO: optimize: if last is an ancestor of BestKnownHeader, continue from there instead.
        if headers.len() == MAX_HEADERS_LEN {
            let start = headers.last().expect("empty checked");
            self.synchronizer
                .send_getheaders_to_peer(self.nc, self.peer, start);
        }
    }
}

#[derive(Clone)]
pub struct HeaderAcceptor<'a, V: Verifier, CI: ChainIndex + 'a> {
    header: &'a Header,
    peer: PeerIndex,
    synchronizer: &'a Synchronizer<CI>,
    resolver: V::Target,
    verifier: V,
}

impl<'a, V, CI> HeaderAcceptor<'a, V, CI>
where
    V: Verifier,
    CI: ChainIndex + 'a,
{
    pub fn new(
        header: &'a Header,
        peer: PeerIndex,
        synchronizer: &'a Synchronizer<CI>,
        resolver: V::Target,
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

    pub fn duplicate_check(&self, state: &mut ValidationResult) -> Result<(), ()> {
        let status = self.synchronizer.get_block_status(&self.header.hash());
        if status != BlockStatus::UNKNOWN {
            if (status & BlockStatus::FAILED_MASK) == status {
                state.invalid(Some(ValidationError::FailedMask));
            }
            if (status & BlockStatus::FAILED_MASK) == status {}
            return Err(());
        }
        Ok(())
    }

    pub fn prev_block_check(&self, state: &mut ValidationResult) -> Result<(), ()> {
        let status = self
            .synchronizer
            .get_block_status(&self.header.parent_hash());

        if (status & BlockStatus::FAILED_MASK) == status {
            state.dos(Some(ValidationError::InvalidParent), 100);
            return Err(());
        }
        Ok(())
    }

    pub fn non_contextual_check(&self, state: &mut ValidationResult) -> Result<(), ()> {
        self.verifier.verify(&self.resolver).map_err(|error| match error {
            VerifyError::Pow(e) => {
                debug!(target: "sync", "HeadersProcess accept {:?} pow", self.header.number());
                state.dos(Some(ValidationError::Verify(VerifyError::Pow(e))), 100);
            }
            VerifyError::Difficulty(e) => {
                debug!(target: "sync", "HeadersProcess accept {:?} difficulty", self.header.number());
                state.dos(
                    Some(ValidationError::Verify(VerifyError::Difficulty(e))),
                    50,
                );
            }
            error => {
                debug!(target: "sync", "HeadersProcess accept {:?} {:?}", self.header.number(), error);
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
        if self.duplicate_check(&mut result).is_err() {
            debug!(target: "sync", "HeadersProcess accept {:?} duplicate", self.header.number());
            return result;
        }

        if self.prev_block_check(&mut result).is_err() {
            debug!(target: "sync", "HeadersProcess accept {:?} prev_block", self.header.number());
            self.synchronizer
                .insert_block_status(self.header.hash(), BlockStatus::FAILED_MASK);
            return result;
        }

        if self.non_contextual_check(&mut result).is_err() {
            debug!(target: "sync", "HeadersProcess accept {:?} non_contextual", self.header.number());
            self.synchronizer
                .insert_block_status(self.header.hash(), BlockStatus::FAILED_MASK);
            return result;
        }

        if self.version_check(&mut result).is_err() {
            debug!(target: "sync", "HeadersProcess accept {:?} version", self.header.number());
            self.synchronizer
                .insert_block_status(self.header.hash(), BlockStatus::FAILED_MASK);
            return result;
        }

        self.synchronizer
            .insert_header_view(&self.header, self.peer);
        self.synchronizer
            .insert_block_status(self.header.hash(), BlockStatus::VALID_MASK);
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

#[derive(Debug, Clone)]
pub enum ValidationError {
    Verify(VerifyError),
    FailedMask,
    Version,
    InvalidParent,
}

#[derive(Debug, Clone, Default)]
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
