use crate::synchronizer::Synchronizer;
use ckb_logger::{debug, info};
use ckb_network::{CKBProtocolContext, PeerIndex};
use failure::Error as FailureError;

pub struct InIBDProcess<'a> {
    synchronizer: &'a Synchronizer,
    peer: PeerIndex,
    nc: &'a dyn CKBProtocolContext,
}

impl<'a> InIBDProcess<'a> {
    pub fn new(
        synchronizer: &'a Synchronizer,
        peer: PeerIndex,
        nc: &'a dyn CKBProtocolContext,
    ) -> Self {
        InIBDProcess {
            nc,
            synchronizer,
            peer,
        }
    }

    pub fn execute(self) -> Result<(), FailureError> {
        info!("getheader with ibd peer {:?}", self.peer);
        if let Some(state) = self.synchronizer.peers().state.write().get_mut(&self.peer) {
            // Don't assume that the peer is sync_started.
            // It is possible that a not-sync-started peer sends us `InIBD` messages:
            //   - Malicious behavior
            //   - Peer sends multiple `InIBD` messages
            if !state.sync_started {
                return Ok(());
            }

            // The node itself needs to ensure the validity of the outbound connection.
            //
            // If outbound is an ibd node(non-whitelist, non-protect), it should be disconnected automatically.
            // If inbound is an ibd node, just mark the node does not pass header sync authentication.
            if state.peer_flags.is_outbound {
                if state.peer_flags.is_whitelist || state.peer_flags.is_protect {
                    self.synchronizer.shared().state().suspend_sync(state);
                } else if let Err(err) = self.nc.disconnect(self.peer, "outbound in ibd") {
                    debug!("synchronizer disconnect error: {:?}", err);
                }
            } else {
                self.synchronizer.shared().state().suspend_sync(state);
            }
        }
        Ok(())
    }
}
