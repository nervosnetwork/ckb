use bigint::H256;
use ckb_chain::chain::ChainProvider;
use ckb_protocol;
use ckb_verification::{Error as VerifyError, HeaderVerifier, Verifier};
use core::header::IndexedHeader;
use log;
use network::{NetworkContext, PeerId};
use protobuf::RepeatedField;
use rayon::prelude::{IntoParallelRefIterator, ParallelIterator};
use synchronizer::{BlockStatus, Synchronizer};
use MAX_HEADERS_LEN;

pub struct HeadersProcess<'a, C: 'a> {
    message: &'a ckb_protocol::Headers,
    synchronizer: &'a Synchronizer<C>,
    peer: PeerId,
    nc: &'a NetworkContext,
}

impl<'a, C> HeadersProcess<'a, C>
where
    C: ChainProvider + 'a,
{
    pub fn new(
        message: &'a ckb_protocol::Headers,
        synchronizer: &'a Synchronizer<C>,
        peer: &PeerId,
        nc: &'a NetworkContext,
    ) -> Self {
        HeadersProcess {
            message,
            nc,
            synchronizer,
            peer: *peer,
        }
    }

    fn is_empty(&self) -> bool {
        self.message.headers.len() == 0
    }

    fn is_oversize(&self) -> bool {
        self.message.headers.len() > MAX_HEADERS_LEN
    }

    fn is_continuous(&self, headers: &[IndexedHeader]) -> bool {
        for window in headers.windows(2) {
            if let [parent, header] = &window {
                if header.parent_hash != parent.hash() {
                    return false;
                }
            }
        }
        true
    }

    fn received_new_header(&self, headers: &[IndexedHeader]) -> bool {
        let last = headers.last().expect("empty checked");
        self.synchronizer.get_block_status(&last.hash()) == BlockStatus::UNKNOWN
    }

    fn push_getheaders(&self, start: &IndexedHeader) {
        let locator_hash = self.synchronizer.get_locator(start);
        let mut payload = ckb_protocol::Payload::new();
        let mut getheaders = ckb_protocol::GetHeaders::new();
        let locator_hash = locator_hash.iter().map(|hash| hash.to_vec()).collect();
        getheaders.set_version(0);
        getheaders.set_block_locator_hashes(RepeatedField::from_vec(locator_hash));
        getheaders.set_hash_stop(H256::default().to_vec());
        payload.set_getheaders(getheaders);
        let _ = self.nc.send(self.peer, payload);
    }

    pub fn accept_first(&self, first: &IndexedHeader) -> ValidationResult {
        if let Some(parent) = self.synchronizer.get_header(&first.parent_hash) {
            let verifier = HeaderVerifier::new(&parent, first, self.synchronizer.ethash.clone());
            let acceptor = HeaderAcceptor::new(first, self.peer, &self.synchronizer, verifier);
            acceptor.accept()
        } else {
            let mut result = ValidationResult::default();
            result.dos(Some(ValidationError::UnknownParent), 10);
            result
        }
    }

    pub fn execute(self) {
        debug!(target: "sync", "HeadersProcess begin");

        if self.is_oversize() {
            self.synchronizer.peers.misbehavior(&self.peer, 20);
            debug!(target: "sync", "HeadersProcess is_oversize");
            return ();
        }

        if self.is_empty() {
            debug!(target: "sync", "HeadersProcess is_empty");
            return ();
        }

        let headers: Vec<IndexedHeader> = self.message.headers.par_iter().map(From::from).collect();

        if !self.is_continuous(&headers) {
            self.synchronizer.peers.misbehavior(&self.peer, 20);
            debug!(target: "sync", "HeadersProcess is not continuous");
            return ();
        }

        let result = self.accept_first(&headers[0]);
        if !result.is_valid() {
            if result.misbehavior > 0 {
                self.synchronizer
                    .peers
                    .misbehavior(&self.peer, result.misbehavior);
                self.synchronizer
                    .insert_block_status(headers[0].hash(), BlockStatus::FAILED_MASK)
            }
            debug!(target: "sync", "\n\nHeadersProcess accept_first is_valid {:?} headers = {:#?}\n\n", result, headers);
            return ();
        }

        for window in headers.windows(2) {
            if let [parent, header] = &window {
                let verifier =
                    HeaderVerifier::new(&parent, &header, self.synchronizer.ethash.clone());
                let acceptor =
                    HeaderAcceptor::new(&header, self.peer, &self.synchronizer, verifier);
                let result = acceptor.accept();

                if !result.is_valid() {
                    if result.misbehavior > 0 {
                        self.synchronizer
                            .peers
                            .misbehavior(&self.peer, result.misbehavior);
                    }

                    self.synchronizer
                        .insert_block_status(header.hash(), BlockStatus::FAILED_MASK);
                    debug!(target: "sync", "HeadersProcess accept is invalid {:?}", result);
                    return ();
                }
            }
        }

        if log_enabled!(target: "sync", log::Level::Debug) {
            let own = { self.synchronizer.best_known_header.read().clone() };
            let chain_tip = { self.synchronizer.chain.tip_header().read().clone() };
            let peer_state = self
                .synchronizer
                .peers
                .best_known_header(&self.peer)
                .unwrap();
            debug!(
                target: "sync",
                "\n\nchain total_difficulty = {}; number={}\n
                number={}; best_known_header = {}; total_difficulty = {};\n\n
                number={}; best_known_header = {}; total_difficulty = {}\n",
                chain_tip.total_difficulty,
                chain_tip.header.number,
                own.header.number,
                own.hash(),
                own.total_difficulty,
                peer_state.header.number,
                peer_state.hash(),
                peer_state.total_difficulty,
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
            self.push_getheaders(&start);
        }
    }
}

#[derive(Clone)]
pub struct HeaderAcceptor<'a, V, C: 'a> {
    header: &'a IndexedHeader,
    peer: PeerId,
    synchronizer: &'a Synchronizer<C>,
    verifier: V,
}

impl<'a, V, C> HeaderAcceptor<'a, V, C>
where
    V: Verifier,
    C: ChainProvider + 'a,
{
    pub fn new(
        header: &'a IndexedHeader,
        peer: PeerId,
        synchronizer: &'a Synchronizer<C>,
        verifier: V,
    ) -> Self {
        HeaderAcceptor {
            header,
            peer,
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
        let status = self.synchronizer.get_block_status(&self.header.parent_hash);

        if status == BlockStatus::UNKNOWN {
            state.dos(Some(ValidationError::UnknownParent), 10);
            return Err(());
        }

        if (status & BlockStatus::FAILED_MASK) == status {
            state.dos(Some(ValidationError::InvalidParent), 100);
            return Err(());
        }
        Ok(())
    }

    pub fn non_contextual_check(&self, state: &mut ValidationResult) -> Result<(), ()> {
        self.verifier.verify().map_err(|error| match error {
            VerifyError::Pow(e) => {
                state.dos(Some(ValidationError::Verify(VerifyError::Pow(e))), 100);
            }
            VerifyError::Difficulty(e) => {
                state.dos(
                    Some(ValidationError::Verify(VerifyError::Difficulty(e))),
                    50,
                );
            }
            _ => (),
        })
    }

    pub fn version_check(&self, state: &mut ValidationResult) -> Result<(), ()> {
        if self.header.version != 0 {
            state.invalid(Some(ValidationError::Version));
            Err(())
        } else {
            Ok(())
        }
    }

    pub fn accept(&self) -> ValidationResult {
        let mut result = ValidationResult::default();
        if self.duplicate_check(&mut result).is_err() {
            return result;
        }

        if self.prev_block_check(&mut result).is_err() {
            return result;
        }

        if self.non_contextual_check(&mut result).is_err() {
            return result;
        }

        if self.version_check(&mut result).is_err() {
            return result;
        }

        self.synchronizer
            .insert_header_view(&self.header, &self.peer);

        result
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum ValidationState {
    VALID,
    INVALID,
    ERROR,
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
    UnknownParent,
    InvalidParent,
}

#[derive(Debug, Clone, Default)]
pub struct ValidationResult {
    pub error: Option<ValidationError>,
    pub misbehavior: u32,
    pub state: ValidationState,
}

impl ValidationResult {
    pub fn error(&mut self, error: Option<ValidationError>) {
        self.error = error;
        self.state = ValidationState::ERROR;
    }

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
