#![no_main]

use libfuzzer_sys::fuzz_target;

use ckb_app_config::NetworkAlertConfig;
use ckb_fuzz::BufManager;
use ckb_network::{bytes::Bytes, SupportProtocols};
use ckb_shared::Shared;
use ckb_sync::SyncShared;
use std::sync::Arc;
use tokio::runtime::Handle;

fn get_proto_type(data: &mut BufManager) -> Result<SupportProtocols, ()> {
    if data.is_end() {
        return Err(());
    }
    let id = data.get::<u8>() % 7;

    // SupportProtocols::Sync => 100,
    // SupportProtocols::RelayV2 => 101,
    // SupportProtocols::RelayV3 => 103,
    // SupportProtocols::Time => 102,
    // SupportProtocols::Alert => 110,
    // SupportProtocols::LightClient => 120,
    // SupportProtocols::Filter => 121,

    match id {
        0 => Ok(SupportProtocols::Sync),
        1 => Ok(SupportProtocols::RelayV2),
        2 => Ok(SupportProtocols::RelayV3),
        3 => Ok(SupportProtocols::Time),
        4 => Ok(SupportProtocols::Alert),
        5 => Ok(SupportProtocols::LightClient),
        6 => Ok(SupportProtocols::Filter),

        _ => Err(()),
    }
}

fn get_shared(data: &mut BufManager, handle: &Handle) -> Result<Shared, ()> {
    if data.is_end() {
        return Err(());
    }
    let builder = ckb_shared::shared_builder::SharedBuilder::new_test(
        ckb_async_runtime::Handle::new(handle.clone(), None),
    );
    let r = builder.build();

    if r.is_err() {
        return Err(());
    }

    Ok(r.unwrap().0)
}

fn get_sync_shared(data: &mut BufManager, handle: &Handle) -> Result<SyncShared, ()> {
    if data.is_end() {
        return Err(());
    }

    let shared = get_shared(data, handle)?;

    let sync_config = ckb_app_config::SyncConfig::default();
    let (_, relay_tx_receiver) = ckb_channel::bounded(0);

    Ok(SyncShared::new(
        shared.clone(),
        sync_config,
        relay_tx_receiver,
    ))
}

fn get_version(data: &mut BufManager) -> Result<ckb_build_info::Version, ()> {
    if data.is_end() {
        return Err(());
    }

    let mut ver = ckb_build_info::Version::default();
    ver.major = data.get();
    ver.minor = data.get();
    ver.patch = data.get();
    Ok(ver)
}

fn get_network_alert_config(data: &mut BufManager) -> Result<NetworkAlertConfig, ()> {
    if data.is_end() {
        return Err(());
    }

    let cfg = NetworkAlertConfig::default();
    Ok(cfg)
}

fn run(data: &[u8]) -> Result<(), ()> {
    // let rt = tokio::runtime::Builder::new_current_thread()
    //     .build()
    //     .unwrap();

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();

    let mut data = BufManager::new(data);

    let t = get_proto_type(&mut data)?;

    let sync_shared = match t {
        SupportProtocols::Time => None,
        _ => Some(Arc::new(get_sync_shared(&mut data, rt.handle())?)),
    };

    let (version, alert_cfg) = match t {
        SupportProtocols::Alert => (
            Some(get_version(&mut data)?),
            Some(get_network_alert_config(&mut data)?),
        ),
        _ => (None, None),
    };

    let proto = ckb_launcher::new_ckb_protocol(t, sync_shared, version, alert_cfg);
    if proto.is_none() {
        return Err(());
    }
    let mut proto = proto.unwrap();

    rt.block_on(async {
        let nc = Arc::new(ckb_fuzz::ckb_protocol_ctx::EmptyProtocolCtx { protocol: 0.into() });

        let _r = proto.init(nc.clone()).await;
        proto.connected(nc.clone(), 0.into(), "").await;
        //
        let bufs = data.get_bufs(0xFFFFFFFF, 7, 1000);
        for buf in bufs {
            proto.received(nc.clone(), 0.into(), Bytes::from(buf)).await;
        }
        proto.disconnected(nc, 0.into()).await;
    });
    Ok(())
}

fuzz_target!(|data: &[u8]| {
    let _r = run(data);
});
