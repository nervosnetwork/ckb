use failure::Fail;

#[derive(Fail, Debug, PartialEq, Eq, Clone)]
pub enum TransactionError {
    /// output.occupied_capacity() > output.capacity()
    // NOTE: the original name is InsufficientCellCapacity
    #[fail(display = "Occupied overflow capacity")]
    OccupiedOverflowCapacity,

    /// SUM([o.capacity for o in outputs]) > SUM([i.capacity for i in inputs])
    // NOTE: the original name is OutputsSumOverflow
    #[fail(display = "Output overflow capacity")]
    OutputOverflowCapacity,

    /// inputs.is_empty() || outputs.is_empty()
    // NOTE: the original name is Empty
    #[fail(display = "Missing inputs or outputs")]
    MissingInputsOrOutputs,

    /// Duplicated dep-out-points within the same one transaction
    // NOTE: the original name is DuplicateDeps
    #[fail(display = "Duplicated deps")]
    DuplicatedDeps,

    /// outputs.len() != outputs_data.len()
    // NOTE: the original name is OutputsDataLengthMismatch
    #[fail(display = "Unmatched outputs-data length with outputs length")]
    UnmatchedOutputsDataLength,

    /// ANY([o.data_hash != d.data_hash() for (o, d) in ZIP(outputs, outputs_data)])
    // NOTE: the original name is OutputDataHashMismatch
    #[fail(display = "Unmatched outputs-data hashes with outputs")]
    UnmatchedOutputsDataHashes,

    /// The format of `transaction.since` is invalid
    // NOTE: the original name is InvalidSince
    #[fail(display = "Invalid Since format")]
    InvalidSinceFormat,

    /// The transaction is not mature which is required by `transaction.since`
    // NOTE: the original name is Immature
    #[fail(display = "Not mature cause of since condition")]
    NotMatureSince,

    /// The transaction is not mature which is required by cellbase maturity rule
    // NOTE: the original name is CellbaseImmaturity
    #[fail(display = "Not mature cause of cellbase-maturity condition")]
    NotMatureCellbase,

    /// The transaction version is mismatched with the system can hold
    #[fail(display = "Mismatched version")]
    MismatchedVersion,

    /// The transaction size is too large
    // NOTE: the original name is ExceededMaximumBlockBytes
    #[fail(display = "Too large size")]
    TooLargeSize,
}
