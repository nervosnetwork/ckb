use super::new_alert_config;
use crate::node::connect_all;
use crate::utils::wait_until;
use crate::{Node, Spec};
use ckb_app_config::{CKBAppConfig, NetworkAlertConfig, RpcModule};
use ckb_crypto::secp::{Message, Privkey};
use ckb_types::{
    bytes::Bytes,
    packed::{self, Alert, RawAlert},
    prelude::*,
};

pub struct AlertPropagation {
    alert_config: NetworkAlertConfig,
    privkeys: Vec<Privkey>,
}

impl Default for AlertPropagation {
    fn default() -> Self {
        let (alert_config, privkeys) = new_alert_config(2, 3);
        Self {
            alert_config,
            privkeys,
        }
    }
}

impl Spec for AlertPropagation {
    crate::setup!(num_nodes: 3);

    // Case: alert propagation in p2p network
    //    1. create and send alert via node0; all nodes should receive the alert;
    //    2. cancel previous alert via node0; all nodes should receive the alert;
    //    3. resend the first alert, all nodes should ignore the alert.
    fn run(&self, nodes: &mut Vec<Node>) {
        connect_all(nodes);

        let node0 = &nodes[0];
        let notice_until = faketime::unix_time_as_millis() + 100_000;

        // create and relay alert
        let id1: u32 = 42;
        let warning1: Bytes = b"pretend we are in dangerous status".to_vec().into();
        let raw_alert = RawAlert::new_builder()
            .id(id1.pack())
            .message(warning1.pack())
            .notice_until(notice_until.pack())
            .build();
        let alert = create_alert(raw_alert, &self.privkeys);
        node0.rpc_client().send_alert(alert.clone().into());
        let ret = wait_until(20, || {
            nodes
                .iter()
                .all(|node| !node.rpc_client().get_blockchain_info().alerts.is_empty())
        });
        assert!(ret, "Alert should be relayed, but not");
        for node in nodes.iter() {
            let alerts = node.rpc_client().get_blockchain_info().alerts;
            assert_eq!(
                alerts.len(),
                1,
                "All nodes should receive the alert, but not"
            );
            assert_eq!(
                alerts[0].message, warning1,
                "Alert message should be {}, but got {}",
                "pretend we are in dangerous status", alerts[0].message
            );
        }

        // cancel previous alert
        let id2: u32 = 43;
        let warning2: Bytes = b"alert is canceled".to_vec().into();
        let raw_alert2 = RawAlert::new_builder()
            .id(id2.pack())
            .cancel(id1.pack())
            .message(warning2.pack())
            .notice_until(notice_until.pack())
            .build();
        let alert2 = create_alert(raw_alert2, &self.privkeys);
        node0.rpc_client().send_alert(alert2.into());
        let ret = wait_until(20, || {
            nodes.iter().all(|node| {
                node.rpc_client()
                    .get_blockchain_info()
                    .alerts
                    .iter()
                    .all(|a| Into::<u32>::into(a.id) != id1)
            })
        });
        assert!(ret, "Alert should be relayed, but not");
        for node in nodes.iter() {
            let alerts = node.rpc_client().get_blockchain_info().alerts;
            assert_eq!(
                alerts.len(),
                1,
                "All nodes should receive the alert, but not"
            );
            assert_eq!(
                alerts[0].message, warning2,
                "Alert message should be {}, but got {}",
                "alert is canceled", alerts[0].message
            );
        }

        // send canceled alert again, should ignore by all nodes
        node0.rpc_client().send_alert(alert.into());
        let ret = wait_until(20, || {
            nodes.iter().all(|node| {
                node.rpc_client()
                    .get_blockchain_info()
                    .alerts
                    .iter()
                    .all(|a| Into::<u32>::into(a.id) != id1)
            })
        });
        assert!(ret, "Alert should be relayed, but not");
        let alerts = node0.rpc_client().get_blockchain_info().alerts;
        assert_eq!(
            alerts.len(),
            1,
            "All nodes should receive the alert, but not"
        );
        assert_eq!(
            alerts[0].message, warning2,
            "Alert message should be {}, but got {}",
            "alert is canceled", alerts[0].message
        );
    }

    fn modify_app_config(&self, config: &mut CKBAppConfig) {
        let alert_config = self.alert_config.to_owned();
        config.network.discovery_local_address = true;
        // set test alert config
        config.alert_signature = Some(alert_config);
        // enable alert RPC
        config.rpc.modules.push(RpcModule::Alert);
    }
}

fn create_alert(raw_alert: RawAlert, privkeys: &[Privkey]) -> Alert {
    let msg: Message = raw_alert.calc_alert_hash().unpack();
    let signatures = privkeys
        .iter()
        .take(2)
        .map(|key| {
            let data: Bytes = key
                .sign_recoverable(&msg)
                .expect("Sign failed")
                .serialize()
                .into();
            data.pack()
        })
        .collect::<Vec<packed::Bytes>>();
    Alert::new_builder()
        .raw(raw_alert)
        .signatures(signatures.pack())
        .build()
}
