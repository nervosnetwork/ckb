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
/// assert_eq!(return_early(StatusCode::OK.into()).to_string(), "OK(10000): bar");
/// assert_eq!(return_early(StatusCode::Ignored.into()).to_string(), "Ignored(10001)");
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
    /// The node has received and recorded this block as pending block
    AlreadyPendingBlock = 102,
    /// The node is requesting from other peers for this block, but no response yet
    AlreadyInFlightBlock = 103,
    /// The node has stored this block into database
    AlreadyStoredBlock = 104,
    /// The CompactBlock is older than what the node expects
    StaledCompactBlock = 105,
    /// The node cannot process the arrived CompactBlock successfully for lack
    /// of information of its parent
    MissingParent = 106,
    /// The node cannot process the arrived CompactBlock successfully for lack
    /// of parts of its transactions
    MissingTransactions = 107,
    /// CompactBlock short-ids collision
    ShortIdsCollided = 108,

    ///////////////////////////////////
    //      Malformed Errors 4xx     //
    ///////////////////////////////////
    /// Malformed protocol message
    MalformedProtocolMessage = 400,
    /// Block verified failed or the block is already marked as invalid
    InvalidBlock = 401,
    /// Header verified failed or the header is already marked as invalid
    InvalidHeader = 402,
    /// Duplicated short-ids within a same CompactBlock
    DuplicatedShortIds = 403,
    /// Missing cellbase as the first transaction within a CompactBlock
    MissingPrefilledCellbase = 404,
    /// Duplicated prefilled transactions within a same CompactBlock
    DuplicatedPrefilledTransactions = 405,
    /// The prefilled transactions are out-of-order
    OutOfOrderPrefilledTransactions = 406,
    /// Some of the prefilled transactions are out-of-index
    OutOfIndexPrefilledTransactions = 407,
    /// Invalid uncle block
    InvalidUncle = 408,
    /// Unmatched Transaction Root
    UnmatchedTransactionRoot = 409,
    /// The length of BlockTransactions is unmatched with in pending_compact_blocks
    UnmatchedBlockTransactionsLength = 410,
    /// The short-ids of BlockTransactions is unmatched with in pending_compact_blocks
    UnmatchedBlockTransactions = 411,
    /// The length of BlockUncles is unmatched with in pending_compact_blocks
    UnmatchedBlockUnclesLength = 412,
    /// The hash of uncles is unmatched
    UnmatchedBlockUncles = 413,
    /// Cannot locate the common blocks based on the GetHeaders
    MissingCommonAncestors = 414,

    ///////////////////////////////////
    //      Warning 5xx              //
    ///////////////////////////////////
    /// Internal undefined error
    Internal = 500,
    /// In-flight blocks limit exceeded
    InflightBlocksReachLimit = 501,
    /// Errors returned from the network layer
    Network = 502,
}

impl StatusCode {
    pub fn with_context<S: ToString>(self, context: S) -> Status {
        Status::new(self, Some(context))
    }
}

#[derive(Clone, Debug, Eq)]
pub struct Status {
    code: StatusCode,
    context: Option<String>,
}

impl Status {
    pub fn new<S: ToString>(code: StatusCode, context: Option<S>) -> Self {
        Self {
            code,
            context: context.map(|s| s.to_string()),
        }
    }

    pub fn ok() -> Self {
        Self::new::<&str>(StatusCode::OK, None)
    }

    pub fn ignored() -> Self {
        Self::new::<&str>(StatusCode::Ignored, None)
    }

    pub fn is_ok(&self) -> bool {
        self.code == StatusCode::OK
    }

    pub fn should_ban(&self) -> Option<Duration> {
        if !(400..500).contains(&(self.code as u16)) {
            return None;
        }
        match self.code {
            StatusCode::MissingCommonAncestors => Some(SYNC_USELESS_BAN_TIME),
            _ => Some(BAD_MESSAGE_BAN_TIME),
        }
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
