use failure::Fail;
use numext_fixed_hash::H256;

#[derive(Fail, Debug, Clone, Eq, PartialEq)]
pub enum SpecError {
    // NOTE: the original name is FileNotFound
    #[fail(display = "ChainSpec: file not found")]
    NotFoundFile,

    // NOTE: the original name is ChainNameNotAllowed
    #[fail(display = "ChainSpec: name not allowed: {}", _0)]
    NotAllowedChainName(String),

    // NOTE: the original name GenesisMismatch
    #[fail(
        display = "ChainSpec: unmatched genesis, expect {:#x} but got {:#x}",
        expect, actual
    )]
    UnmatchedGenesis { expect: H256, actual: H256 },
}
