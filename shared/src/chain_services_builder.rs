//! chain_services_builder provide ChainServicesBuilder to build Chain Services
#![allow(missing_docs)]
use crate::Shared;
use ckb_proposal_table::ProposalTable;

pub struct ChainServicesBuilder {
    pub shared: Shared,
    pub proposal_table: ProposalTable,
}

impl ChainServicesBuilder {
    pub fn new(shared: Shared, proposal_table: ProposalTable) -> Self {
        ChainServicesBuilder {
            shared,
            proposal_table,
        }
    }
}
