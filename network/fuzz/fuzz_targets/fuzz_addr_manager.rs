#![no_main]

use libfuzzer_sys::fuzz_target;

use ckb_network::{
    peer_store::{addr_manager::AddrManager, types::AddrInfo},
    PeerId,
};

use std::collections::HashSet;
use std::net::Ipv4Addr;

const ADDR_SIZE: usize = 4 + 32; // IPv4+Port+SHA256

fn new_addr(data: &[u8], index: usize) -> AddrInfo {
    let pos = 8 + index * ADDR_SIZE;
    let data = data[pos..pos + ADDR_SIZE].to_vec();

    let ip = Ipv4Addr::from(u32::from_le_bytes(data[0..4].try_into().unwrap()));
    // let ip = Ipv4Addr::from(((225 << 24) + index) as u32);
    // let port = u16::from_le_bytes(data[4..6].try_into().unwrap());
    let peer_id =
        PeerId::from_bytes([vec![0x12], vec![0x20], data[4..].to_vec()].concat()).unwrap();

    AddrInfo::new(
        format!("/ip4/{}/tcp/43/p2p/{}", ip, peer_id.to_base58())
            .parse()
            .unwrap(),
        0,
        0,
        0,
    )
}

fn test_remove(
    mut addr_manager: AddrManager,
    basic: &HashSet<AddrInfo>,
    rm_num: usize,
) -> AddrManager {
    let removed = addr_manager.fetch_random(rm_num, |_| true);
    // assert_eq!(removed.len(), rm_num.min(basic.len()));
    assert!(removed.len() <= rm_num.min(basic.len()));

    for addr in &removed {
        addr_manager.remove(&addr.addr);
    }
    assert!(addr_manager.count() <= (basic.len() - removed.len()));
    for addr in removed {
        addr_manager.add(addr);
    }
    assert!(addr_manager.count() <= basic.len());

    addr_manager
}

fuzz_target!(|data: &[u8]| {
    if data.len() < 8 + ADDR_SIZE {
        return;
    }
    if (data.len() - 8) % ADDR_SIZE != 0 {
        return;
    }
    let scale: u16 = u16::from_le_bytes(data[0..2].try_into().unwrap()) % 100;
    let (basic_len, added_len) = {
        let t = (data.len() - 8) / ADDR_SIZE;

        let b = (t as f32 / 100.0 * scale as f32) as usize;
        (b, t - b)
    };
    let mut addr_manager = AddrManager::default();

    let mut basic = HashSet::new();
    for i in 0..basic_len {
        let addr = new_addr(data, i);
        basic.insert(addr.clone());
        addr_manager.add(addr);
    }
    assert!(basic.len() >= addr_manager.count());

    let removed_num1 =
        u16::from_le_bytes(data[2..4].try_into().unwrap()) as usize % (basic.len() + 8);
    let mut addr_manager = test_remove(addr_manager, &basic, removed_num1);

    let mut added = Vec::new();
    for i in 0..added_len {
        let addr = new_addr(data, i + basic_len);
        added.push(addr.clone());

        addr_manager.add(addr.clone());
        basic.insert(addr);
    }
    assert!(basic.len() >= addr_manager.count());

    let removed_num2 =
        u16::from_le_bytes(data[4..6].try_into().unwrap()) as usize % (basic.len() + 4);

    let mut _addr_manager = test_remove(addr_manager, &basic, removed_num2);
});
