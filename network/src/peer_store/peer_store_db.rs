use crate::{
    errors::{Error, PeerStoreError},
    peer_store::{
        addr_manager::AddrManager,
        ban_list::BanList,
        types::{AddrInfo, BannedAddr},
        PeerStore,
    },
};
use ckb_logger::debug;
use std::fs::{create_dir_all, File, OpenOptions};
use std::path::Path;

const DEFAULT_ADDR_MANAGER_DB: &str = "addr_manager.db";
const DEFAULT_BAN_LIST_DB: &str = "ban_list.db";

impl AddrManager {
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self, Error> {
        let file = File::open(path)?;
        let addrs: Vec<AddrInfo> = serde_json::from_reader(file).map_err(PeerStoreError::Serde)?;
        let mut addr_manager = AddrManager::default();
        addrs.into_iter().for_each(|addr| addr_manager.add(addr));
        Ok(addr_manager)
    }

    pub fn dump<P: AsRef<Path>>(&self, path: P) -> Result<(), Error> {
        let addrs: Vec<_> = self.addrs_iter().collect();
        if let Some(dir) = path.as_ref().parent() {
            create_dir_all(dir)?;
        }
        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .append(false)
            .open(path)?;
        serde_json::to_writer(file, &addrs).map_err(|err| PeerStoreError::Serde(err).into())
    }
}

impl BanList {
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self, Error> {
        let file = File::open(path)?;
        let banned_addrs: Vec<BannedAddr> =
            serde_json::from_reader(file).map_err(PeerStoreError::Serde)?;
        let mut ban_list = BanList::default();
        banned_addrs
            .into_iter()
            .for_each(|banned_addr| ban_list.ban(banned_addr));
        Ok(ban_list)
    }

    pub fn dump<P: AsRef<Path>>(&self, path: P) -> Result<(), Error> {
        let banned_addrs = self.get_banned_addrs();
        if let Some(dir) = path.as_ref().parent() {
            create_dir_all(dir)?;
        }
        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .append(false)
            .open(path)?;
        serde_json::to_writer(file, &banned_addrs).map_err(|err| PeerStoreError::Serde(err).into())
    }
}

impl PeerStore {
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self, Error> {
        let addr_manager_path = path.as_ref().join(DEFAULT_ADDR_MANAGER_DB);
        let ban_list_path = path.as_ref().join(DEFAULT_BAN_LIST_DB);

        let addr_manager = if addr_manager_path.exists() {
            AddrManager::load(addr_manager_path)?
        } else {
            debug!("Failed to load addr manager from {:?}", addr_manager_path);
            AddrManager::default()
        };

        let ban_list = if ban_list_path.exists() {
            BanList::load(ban_list_path)?
        } else {
            debug!("Failed to load ban list from {:?}", ban_list_path);
            BanList::default()
        };

        Ok(PeerStore::new(addr_manager, ban_list))
    }

    pub fn dump<P: AsRef<Path>>(&self, path: P) -> Result<(), Error> {
        self.addr_manager()
            .dump(path.as_ref().join(DEFAULT_ADDR_MANAGER_DB))?;
        self.ban_list()
            .dump(path.as_ref().join(DEFAULT_BAN_LIST_DB))?;
        Ok(())
    }
}
