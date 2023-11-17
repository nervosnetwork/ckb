use ckb_channel::Receiver;
use ckb_proposal_table::ProposalTable;
use ckb_tx_pool::service::TxVerificationResult;
use ckb_tx_pool::TxPoolServiceBuilder;

/// SharedBuilder build returning the shared/package halves
/// The package structs used for init other component
pub struct SharedPackage {
    table: Option<ProposalTable>,
    tx_pool_builder: Option<TxPoolServiceBuilder>,
    relay_tx_receiver: Option<Receiver<TxVerificationResult>>,
}

impl SharedPackage {
    /// Takes the proposal_table out of the package, leaving a None in its place.
    pub fn take_proposal_table(&mut self) -> ProposalTable {
        self.table.take().expect("take proposal_table")
    }

    /// Takes the tx_pool_builder out of the package, leaving a None in its place.
    pub fn take_tx_pool_builder(&mut self) -> TxPoolServiceBuilder {
        self.tx_pool_builder.take().expect("take tx_pool_builder")
    }

    /// Takes the relay_tx_receiver out of the package, leaving a None in its place.
    pub fn take_relay_tx_receiver(&mut self) -> Receiver<TxVerificationResult> {
        self.relay_tx_receiver
            .take()
            .expect("take relay_tx_receiver")
    }
}
