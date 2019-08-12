use std::fmt::{self, Display, Formatter};

/// Similar to `?`, `attempt!` is used for propagating `Status`.
///
/// `attempt!` accepts two types of expressions:
///
/// - `Result<T, E>`: return `Status` transformed from `E` for `Err(E)`,
///   unwrap entity `T` for `Ok(T)`.
/// - `Status`: return `Status` if it is not ok, otherwise the original `Status`
///
/// ```rust
/// use ckb_protocol::error::Error;
/// use ckb_sync::{Status, StatusCode, attempt};
///
/// fn unwrap_for_result() -> Status {
///     let result = Ok(StatusCode::Ignored);
///     let code = attempt!(result); // !-> `let code = result.unwrap();`
///     code
/// }
///
/// fn return_early_for_result() -> Status {
///     attempt!(Err(Error::Malformed)); // !-> `return Status::new(StatusCode::MalformedProtocolMessage);`
///     Status::ok()
/// }
///
/// fn return_early_for_status() -> Status {
///     let status = StatusCode::MalformedProtocolMessage.into();
///     attempt!(status); // !-> `return Status::new(StatusCode::MalformedProtocolMessage);`
///     Status::ok()
/// }
/// ```
#[macro_export]
macro_rules! attempt {
    ($code:expr) => {
        if $code.is_ok() {
            $code.unwrap()
        } else {
            return $code.unwrap_err().into();
        }
    };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusCode {
    ///////////////////////////////////
    //      Informational 1xx         //
    ///////////////////////////////////
    /// The node has processed the message successfully
    OK = 100,
    /// Ignore the message, normally because already has processed or this
    /// message is staled
    Ignored = 101,
    /// The node has received and recorded this block as pending block
    AlreadyPendingBlock = 102,
    /// The node is requesting from other peers for this block, but no response yet
    AlreadyInFlightBlock = 103,
    /// The node has stored this block into database
    AlreadyStoredBlock = 104,
    /// The CompactBlock is older than what the node expects
    TooOldBlock = 105,
    /// The node cannot process the arrived CompactBlock successfully for lack
    /// of information of its parent
    WaitingParent = 106,
    /// The node cannot process the arrived CompactBlock successfully for lack
    /// of parts of its transactions
    WaitingTransactions = 107,

    ///////////////////////////////////
    //      Malformed Error 4xx      //
    ///////////////////////////////////
    MalformedProtocolMessage = 401,
    /// Duplicated short-ids within a same CompactBlock
    DuplicatedShortIds = 402,
    /// Missing cellbase as the first transaction within a CompactBlock
    MissingPrefilledCellbase = 403,
    /// Duplicated prefilled transactions within a same CompactBlock
    DuplicatedPrefilledTransactions = 404,
    /// The prefilled transactions are out-of-order
    OutOfOrderPrefilledTransactions = 405,
    /// Some of the prefilled transactions are out-of-index
    OutOfIndexPrefilledTransactions = 406,
    /// The length of BlockTransactions is unmatched with in pending_compact_blocks
    UnmatchedBlockTransactionsLength = 407,
    /// The short-ids of BlockTransactions is unmatched with in pending_compact_blocks
    UnmatchedBlockTransactions = 408,
    /// Invalid block
    InvalidBlock = 409,
    /// Invalid header
    InvalidHeader = 410,

    ///////////////////////////////////
    //      Server Error 5xx         //
    ///////////////////////////////////
    /// In-flight blocks limit exceeded
    TooManyInFlightBlocks = 501,
}

impl StatusCode {
    pub fn with_context(self, context: String) -> Status {
        Status::with_context(self, context)
    }
}

#[derive(Clone, Debug, Eq)]
pub struct Status {
    pub code: StatusCode,
    pub context: Option<String>,
}

impl Status {
    pub fn ok() -> Self {
        Self::new(StatusCode::OK)
    }

    pub fn ignored() -> Self {
        Self::new(StatusCode::Ignored)
    }

    pub fn new(code: StatusCode) -> Self {
        Self {
            code,
            context: None,
        }
    }

    pub fn with_context(code: StatusCode, context: String) -> Self {
        Self {
            code,
            context: Some(context),
        }
    }

    pub fn is_ok(&self) -> bool {
        self.code == StatusCode::OK
    }

    pub fn is_malformed(&self) -> bool {
        400 <= self.code as u16 && (self.code as u16) < 500
    }

    pub fn unwrap_err(self) -> Self {
        self
    }

    pub fn unwrap(self) -> Self {
        self
    }
}

// We treat two `Status` are equal if they has the same status code.
impl PartialEq for Status {
    fn eq(&self, other: &Self) -> bool {
        self.code == other.code
    }
}

impl Display for Status {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match self.context {
            Some(ref context) => write!(f, "{:?}({})", self.code, context),
            None => write!(f, "{:?}", self.code),
        }
    }
}

impl From<StatusCode> for Status {
    fn from(code: StatusCode) -> Self {
        Self {
            code,
            context: None,
        }
    }
}

impl From<ckb_protocol::error::Error> for Status {
    fn from(error: ckb_protocol::error::Error) -> Self {
        match error {
            ckb_protocol::error::Error::Malformed => StatusCode::MalformedProtocolMessage.into(),
        }
    }
}

// bilibili FIXME
impl From<failure::Error> for Status {
    fn from(_error: failure::Error) -> Self {
        StatusCode::MalformedProtocolMessage.into()
    }
}
