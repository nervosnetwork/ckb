use crate::{BAD_MESSAGE_BAN_TIME, SYNC_USELESS_BAN_TIME};
use std::fmt::{self, Display, Formatter};
use std::time::Duration;

/// Similar to `?`, `attempt!` is used for propagating `Status`.
///
/// `attempt!` return early if it is not `Status::ok()`.
///
/// ```rust
/// use ckb_sync::{Status, StatusCode, attempt};
///
/// fn return_early(status: Status) -> Status {
///     attempt!(status);
///     StatusCode::OK.with_context("bar")
/// }
///
/// assert_eq!(return_early(StatusCode::OK.into()).to_string(), "OK(100): bar");
/// assert_eq!(return_early(StatusCode::Ignored.into()).to_string(), "Ignored(101)");
/// ```
#[macro_export]
macro_rules! attempt {
    ($code:expr) => {{
        let ret = $code;
        if !ret.is_ok() {
            return ret;
        }
        ret
    }};
}

/// StatusCodes indicate whether a specific operation has been successfully completed.
/// The StatusCode element is a 3-digit integer.
///
/// The first digest of the StatusCode defines the class of result:
///   - 1xx: Informational - Request received, continuing process
///   - 4xx: Malformed Error - The request contains malformed messages
///   - 5xx: Warning - The node warns about recoverable conditions
#[repr(u16)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusCode {
    ///////////////////////////////////
    //      Informational 1xx        //
    ///////////////////////////////////
    /// OK
    OK = 100,
    /// Ignored
    Ignored = 101,
    /// The node had already received and recorded this block as pending block
    CompactBlockIsAlreadyPending = 102,
    /// The node is requesting from other peers for this block, but no response yet
    CompactBlockIsAlreadyInFlight = 103,
    /// The node had already stored this block into database
    CompactBlockAlreadyStored = 104,
    /// The CompactBlock is older than what the node expects
    CompactBlockIsStaled = 105,
    /// The node cannot process the arrived CompactBlock successfully for lack
    /// of information of its parent
    CompactBlockRequiresParent = 106,
    /// The node cannot process the arrived CompactBlock successfully for lack
    /// of parts of its transactions
    CompactBlockRequiresFreshTransactions = 107,
    /// CompactBlock short-ids collision
    CompactBlockMeetsShortIdsCollision = 108,

    ///////////////////////////////////
    //      Malformed Errors 4xx     //
    ///////////////////////////////////
    /// Malformed protocol message
    ProtocolMessageIsMalformed = 400,
    /// Block verified failed or the block is already marked as invalid
    BlockIsInvalid = 401,
    /// Header verified failed or the header is already marked as invalid
    CompactBlockHasInvalidHeader = 402,
    /// Duplicated short-ids within a same CompactBlock
    CompactBlockHasDuplicatedShortIds = 403,
    /// Missing cellbase as the first transaction within a CompactBlock
    CompactBlockHasNotPrefilledCellbase = 404,
    /// Duplicated prefilled transactions within a same CompactBlock
    CompactBlockHasDuplicatedPrefilledTransactions = 405,
    /// The prefilled transactions are out-of-order
    CompactBlockHasOutOfOrderPrefilledTransactions = 406,
    /// Some of the prefilled transactions are out-of-index
    CompactBlockHasOutOfIndexPrefilledTransactions = 407,
    /// Invalid uncle block
    CompactBlockHasInvalidUncle = 408,
    /// Unmatched Transaction Root
    CompactBlockHasUnmatchedTransactionRootWithReconstructedBlock = 409,
    /// The length of BlockTransactions is unmatched with in pending_compact_blocks
    BlockTransactionsLengthIsUnmatchedWithPendingCompactBlock = 410,
    /// The short-ids of BlockTransactions is unmatched with in pending_compact_blocks
    BlockTransactionsShortIdsAreUnmatchedWithPendingCompactBlock = 411,
    /// The length of BlockUncles is unmatched with in pending_compact_blocks
    BlockUnclesLengthIsUnmatchedWithPendingCompactBlock = 412,
    /// The hash of uncles is unmatched
    BlockUnclesAreUnmatchedWithPendingCompactBlock = 413,
    /// Cannot locate the common blocks based on the GetHeaders
    GetHeadersMissCommonAncestors = 414,

    /// Generic rate limit error
    TooManyRequests = 429,

    ///////////////////////////////////
    //      Warning 5xx              //
    ///////////////////////////////////
    /// Errors returned from the tx-pool
    TxPool = 501,
    /// Errors returned from the network layer
    Network = 502,
    /// In-flight blocks limit exceeded
    BlocksInFlightReachLimit = 503,
}

impl StatusCode {
    /// TODO(doc): @driftluo
    pub fn with_context<S: ToString>(self, context: S) -> Status {
        Status::new(self, Some(context))
    }
}

/// TODO(doc): @driftluo
#[derive(Clone, Debug, Eq)]
pub struct Status {
    code: StatusCode,
    context: Option<String>,
}

impl Status {
    /// TODO(doc): @driftluo
    pub fn new<S: ToString>(code: StatusCode, context: Option<S>) -> Self {
        Self {
            code,
            context: context.map(|s| s.to_string()),
        }
    }

    /// TODO(doc): @driftluo
    pub fn ok() -> Self {
        Self::new::<&str>(StatusCode::OK, None)
    }

    /// TODO(doc): @driftluo
    pub fn ignored() -> Self {
        Self::new::<&str>(StatusCode::Ignored, None)
    }

    /// TODO(doc): @driftluo
    pub fn is_ok(&self) -> bool {
        self.code == StatusCode::OK
    }

    /// TODO(doc): @driftluo
    pub fn should_ban(&self) -> Option<Duration> {
        if !(400..500).contains(&(self.code as u16)) {
            return None;
        }
        match self.code {
            StatusCode::GetHeadersMissCommonAncestors => Some(SYNC_USELESS_BAN_TIME),
            _ => Some(BAD_MESSAGE_BAN_TIME),
        }
    }

    /// TODO(doc): @driftluo
    pub fn should_warn(&self) -> bool {
        self.code as u16 >= 500
    }

    /// TODO(doc): @driftluo
    pub fn code(&self) -> StatusCode {
        self.code
    }

    pub(crate) fn tag(&self) -> String {
        format!("{:?}", self.code)
    }
}

impl PartialEq for Status {
    fn eq(&self, other: &Self) -> bool {
        self.code == other.code
    }
}

impl Display for Status {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match self.context {
            Some(ref context) => write!(f, "{:?}({}): {}", self.code, self.code as u16, context),
            None => write!(f, "{:?}({})", self.code, self.code as u16),
        }
    }
}

impl From<StatusCode> for Status {
    fn from(code: StatusCode) -> Self {
        Self::new::<&str>(code, None)
    }
}
