use ckb_error::{prelude::*, Error, ErrorKind};
use ckb_types::core::CapacityError;

/// Errors due to the fact that the NervosDAO rules are not respected.
///
/// [NervosDAO]: https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0023-dao-deposit-withdraw/0023-dao-deposit-withdraw.md
#[derive(Error, Debug, PartialEq, Clone, Eq)]
pub enum DaoError {
    /// This error occurs during calculating the dao field for a block, which broadly indicates that it cannot find a required block.
    #[error("InvalidHeader")]
    InvalidHeader,

    /// When withdraws from NervosDAO, it requires the deposited header and withdrawing header to help calculating interest.
    /// This error occurs at [withdrawing phase 2] for the below cases:
    ///   - The `HeaderDeps` does not include the withdrawing block hash. The withdrawing block hash
    ///     indicates the block which packages the target transaction at [withdrawing phase 1].
    ///   - The `HeaderDeps` does not include the deposited block hash. The deposited block hash
    ///     indicates the block which packages the target transaction at [deposit phase]. Please see
    ///     [withdrawing phase 2] for more details.
    ///
    /// [deposit phase]: https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0023-dao-deposit-withdraw/0023-dao-deposit-withdraw.md#deposit
    /// [withdrawing phase 1]: https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0023-dao-deposit-withdraw/0023-dao-deposit-withdraw.md#withdraw-phase-1
    /// [withdrawing phase 2]: https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0023-dao-deposit-withdraw/0023-dao-deposit-withdraw.md#withdraw-phase-2
    #[error("InvalidOutPoint")]
    InvalidOutPoint,

    /// When withdraws from NervosDAO, the deposited header should be specified via witness. This error
    /// indicates the corresponding witness is unexpected. See
    /// [the code](https://github.com/nervosnetwork/ckb/blob/69ff8311cdb312a0ef45d524060719eea5e90e9e/util/dao/src/lib.rs#L280-L301)
    /// for more detail.
    ///
    /// See also:
    /// - [0023-dao-deposit-withdraw](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0023-dao-deposit-withdraw/0023-dao-deposit-withdraw.md)
    #[error("InvalidDaoFormat")]
    InvalidDaoFormat,
    /// Calculation overflow
    #[error("Overflow")]
    Overflow,
    /// ZeroC
    #[error("ZeroC")]
    ZeroC,
}

impl From<DaoError> for Error {
    fn from(error: DaoError) -> Self {
        ErrorKind::Dao.because(error)
    }
}

impl From<CapacityError> for DaoError {
    fn from(error: CapacityError) -> Self {
        match error {
            CapacityError::Overflow => DaoError::Overflow,
        }
    }
}
