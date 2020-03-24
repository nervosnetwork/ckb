use crate::utils::wait_until;
use crate::{Net, Spec, TestProtocol};
use ckb_sync::{NetworkProtocol, MAX_LOCATOR_SIZE};
use ckb_types::{
    h256,
    packed::{Byte32, GetHeaders, SyncMessage},
    prelude::*,
    H256,
};
use log::info;

pub struct InvalidLocatorSize;

impl Spec for InvalidLocatorSize {
    crate::name!("invalid_locator_size");

    crate::setup!(protocols: vec![TestProtocol::sync()]);

    fn run(&self, net: &mut Net) {
        info!("Connect node0");
        net.exit_ibd_mode();
        let node0 = &net.nodes[0];
        net.connect(node0);
        // get peer_id from GetHeaders message
        let (peer_id, _, _) = net.receive();

        let hashes: Vec<Byte32> = (0..=MAX_LOCATOR_SIZE)
            .map(|_| h256!("0x1").pack())
            .collect();

        let message = SyncMessage::new_builder()
            .set(
                GetHeaders::new_builder()
                    .block_locator_hashes(hashes.pack())
                    .build(),
            )
            .build()
            .as_bytes();

        net.send(NetworkProtocol::SYNC.into(), peer_id, message);

        let rpc_client = net.nodes[0].rpc_client();
        let ret = wait_until(10, || rpc_client.get_peers().is_empty());
        assert!(ret, "Node0 should disconnect test node");

        net.connect(node0);
        let ret = wait_until(10, || !rpc_client.get_peers().is_empty());
        assert!(!ret, "Node0 should ban test node");
    }
}
