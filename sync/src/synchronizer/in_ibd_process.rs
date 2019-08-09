use crate::synchronizer::Synchronizer;
use crate::PROTECT_STOP_SYNC_TIME;
use ckb_logger::{debug, info};
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_protocol::InIBD;
use failure::Error as FailureError;
use faketime::unix_time_as_millis;
use std::sync::atomic::Ordering;

pub struct InIBDProcess<'a> {
    _message: &'a InIBD<'a>,
    synchronizer: &'a Synchronizer,
    peer: PeerIndex,
    nc: &'a CKBProtocolContext,
}

impl<'a> InIBDProcess<'a> {
    pub fn new(
        _message: &'a InIBD,
        synchronizer: &'a Synchronizer,
        peer: PeerIndex,
        nc: &'a CKBProtocolContext,
    ) -> Self {
        InIBDProcess {
            _message,
            nc,
            synchronizer,
            peer,
        }
    }

    pub fn execute(self) -> Result<(), FailureError> {
        info!("getheader with ibd peer {:?}", self.peer);
        if let Some(state) = self
            .synchronizer
            .shared
            .peers()
            .state
            .write()
            .get_mut(&self.peer)
        {
            let now = unix_time_as_millis();
            // The node itself needs to ensure the validity of the outbound connection.
            //
            // If outbound is a ibd node(non-whitelist, non-protect), it should be disconnected automatically.
            // If inbound is a ibd node, just mark the node does not pass header sync authentication.
            if state.peer_flags.is_outbound {
                if state.peer_flags.is_whitelist || state.peer_flags.is_protect {
                    state.stop_sync(now + PROTECT_STOP_SYNC_TIME);
                    self.synchronizer
                        .shared()
                        .n_sync_started()
                        .fetch_sub(1, Ordering::Release);
                } else if let Err(err) = self.nc.disconnect(self.peer, "outbound in ibd") {
                    debug!("synchronizer disconnect error: {:?}", err);
                }
            } else {
                state.stop_sync(now + PROTECT_STOP_SYNC_TIME);
                self.synchronizer
                    .shared()
                    .n_sync_started()
                    .fetch_sub(1, Ordering::Release);
            }
        }
        Ok(())
    }
}
