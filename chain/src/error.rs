use shared::error::SharedError;
use verification::Error as VerifyError;

#[derive(Debug, PartialEq, Clone, Eq)]
pub enum ProcessBlockError {
    Shared(SharedError),
    Verification(VerifyError),
}
