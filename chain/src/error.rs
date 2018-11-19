use ckb_shared::error::SharedError;
use ckb_verification::Error as VerifyError;

#[derive(Debug, PartialEq, Clone, Eq)]
pub enum ProcessBlockError {
    Shared(SharedError),
    Verification(VerifyError),
}
