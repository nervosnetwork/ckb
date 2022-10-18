mod get_blocks_proof;
mod get_last_state;
mod get_last_state_proof;
mod get_transactions_proof;

#[cfg(test)]
mod tests;

pub(crate) use get_blocks_proof::GetBlocksProofProcess;
pub(crate) use get_last_state::GetLastStateProcess;
pub(crate) use get_last_state_proof::GetLastStateProofProcess;
pub(crate) use get_transactions_proof::GetTransactionsProofProcess;
