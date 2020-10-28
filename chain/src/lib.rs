//! CKB chain service.
//!
//! [ChainService] background base on database, handle block importing,
//! the [ChainController] is responsible for receive the request and returning response
//!
//! [ChainService](chain/struct.ChainService.html)
//! [ChainController](chain/struct.ChainController.html)

pub mod chain;
pub mod switch;
#[cfg(test)]
mod tests;
