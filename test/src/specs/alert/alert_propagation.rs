use super::new_alert_config;
use crate::utils::wait_until;
use crate::{Net, Spec};
use ckb_app_config::{CKBAppConfig, NetworkAlertConfig, RpcModule};
use ckb_crypto::secp::{Message, Privkey};
use ckb_types::{
    bytes::Bytes,
    packed::{self, Alert, RawAlert},
    prelude::*,
};
use log::info;

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
    crate::name!("alert_propagation");

    crate::setup!(num_nodes: 3);

    fn run(&self, net: &mut Net) {
        let node0 = &net.nodes[0];
        let warning1: Bytes = b"pretend we are in dangerous status".to_vec().into();
        let id1: u32 = 42;
        let notice_until = faketime::unix_time_as_millis() + 100_000;

        // send alert
        let raw_alert = RawAlert::new_builder()
            .id(id1.pack())
            .message(warning1.pack())
            .notice_until(notice_until.pack())
            .build();
        let msg: Message = raw_alert.calc_alert_hash().unpack();
        let signatures = self
            .privkeys
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
        let alert = Alert::new_builder()
            .raw(raw_alert)
            .signatures(signatures.pack())
            .build();
        // send alert
        node0.rpc_client().send_alert(alert.clone().into());
        info!("Waiting for alert relay");
        let ret = wait_until(20, || {
            net.nodes
                .iter()
                .all(|node| !node.rpc_client().get_blockchain_info().alerts.is_empty())
        });
        assert!(ret, "alert is relayed");
        for node in net.nodes.iter() {
            let alerts = node.rpc_client().get_blockchain_info().alerts;
            assert_eq!(alerts.len(), 1);
            assert_eq!(alerts[0].message, warning1);
        }

        // cancel previous alert
        let warning2: Bytes = b"alert is canceled".to_vec().into();
        let raw_alert2 = RawAlert::new_builder()
            .id(2u32.pack())
            .cancel(id1.pack())
            .message(warning2.pack())
            .notice_until(notice_until.pack())
            .build();
        let msg: Message = raw_alert2.calc_alert_hash().unpack();
        let signatures = self
            .privkeys
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
        let alert2 = Alert::new_builder()
            .raw(raw_alert2)
            .signatures(signatures.pack())
            .build();
        node0.rpc_client().send_alert(alert2.into());
        info!("Waiting for alert relay");
        let ret = wait_until(20, || {
            net.nodes.iter().all(|node| {
                node.rpc_client()
                    .get_blockchain_info()
                    .alerts
                    .iter()
                    .all(|a| Into::<u32>::into(a.id) != id1)
            })
        });
        assert!(ret, "alert is relayed");
        for node in net.nodes.iter() {
            let alerts = node.rpc_client().get_blockchain_info().alerts;
            assert_eq!(alerts.len(), 1);
            assert_eq!(alerts[0].message, warning2);
        }

        // send canceled alert again, should ignore by all nodes
        node0.rpc_client().send_alert(alert.into());
        let alerts = node0.rpc_client().get_blockchain_info().alerts;
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].message, warning2);
    }

    fn modify_ckb_config(&self) -> Box<dyn Fn(&mut CKBAppConfig) -> ()> {
        let alert_config = self.alert_config.to_owned();
        Box::new(move |config| {
            config.network.discovery_local_address = true;
            // set test alert config
            config.alert_signature = Some(alert_config.clone());
            // enable alert RPC
            config.rpc.modules.push(RpcModule::Alert);
        })
    }
}
