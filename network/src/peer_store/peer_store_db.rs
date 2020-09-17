use crate::{
    errors::{Error, PeerStoreError},
    peer_store::{
        addr_manager::AddrManager,
        ban_list::BanList,
        types::{AddrInfo, BannedAddr},
        PeerStore,
    },
};
use ckb_logger::{debug, error};
use std::fs::{copy, create_dir_all, remove_file, rename, File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;

const DEFAULT_ADDR_MANAGER_DB: &str = "addr_manager.db";
const DEFAULT_BAN_LIST_DB: &str = "ban_list.db";

impl AddrManager {
    pub fn load<R: Read>(r: R) -> Result<Self, Error> {
        let addrs: Vec<AddrInfo> = serde_json::from_reader(r).map_err(PeerStoreError::Serde)?;
        let mut addr_manager = AddrManager::default();
        addrs.into_iter().for_each(|addr| addr_manager.add(addr));
        Ok(addr_manager)
    }

    pub fn dump<W: Write>(&self, w: W) -> Result<(), Error> {
        let addrs: Vec<_> = self.addrs_iter().collect();
        debug!("dump {} addrs", addrs.len());
        serde_json::to_writer(w, &addrs).map_err(|err| PeerStoreError::Serde(err).into())
    }
}

impl BanList {
    pub fn load<R: Read>(r: R) -> Result<Self, Error> {
        let banned_addrs: Vec<BannedAddr> =
            serde_json::from_reader(r).map_err(PeerStoreError::Serde)?;
        let mut ban_list = BanList::default();
        banned_addrs
            .into_iter()
            .for_each(|banned_addr| ban_list.ban(banned_addr));
        Ok(ban_list)
    }

    pub fn dump<W: Write>(&self, w: W) -> Result<(), Error> {
        let banned_addrs = self.get_banned_addrs();
        debug!("dump {} banned addrs", banned_addrs.len());
        serde_json::to_writer(w, &banned_addrs).map_err(|err| PeerStoreError::Serde(err).into())
    }
}

impl PeerStore {
    pub fn load_from_dir_or_default<P: AsRef<Path>>(path: P) -> Self {
        let addr_manager_path = path.as_ref().join(DEFAULT_ADDR_MANAGER_DB);
        let ban_list_path = path.as_ref().join(DEFAULT_BAN_LIST_DB);

        let addr_manager = File::open(&addr_manager_path)
            .map_err(|err| {
                debug!(
                    "Failed to open AddrManager db, file: {:?}, error: {:?}",
                    addr_manager_path, err
                )
            })
            .and_then(|file| {
                AddrManager::load(file).map_err(|err| {
                    error!(
                        "Failed to load AddrManager db, file: {:?}, error: {:?}",
                        addr_manager_path, err
                    )
                })
            })
            .unwrap_or_default();

        let ban_list = File::open(&ban_list_path)
            .map_err(|err| {
                debug!(
                    "Failed to open BanList db, file: {:?}, error: {:?}",
                    ban_list_path, err
                )
            })
            .and_then(|file| {
                BanList::load(file).map_err(|err| {
                    error!(
                        "Failed to load BanList db, file: {:?}, error: {:?}",
                        ban_list_path, err
                    )
                })
            })
            .unwrap_or_default();

        PeerStore::new(addr_manager, ban_list)
    }

    pub fn dump_to_dir<P: AsRef<Path>>(&self, path: P) -> Result<(), Error> {
        // create dir
        create_dir_all(&path)?;
        // dump file to a temporary sub-directory
        let tmp_dir = path.as_ref().join("tmp");
        create_dir_all(&tmp_dir)?;
        let tmp_addr_manager = tmp_dir.join(DEFAULT_ADDR_MANAGER_DB);
        let tmp_ban_list = tmp_dir.join(DEFAULT_BAN_LIST_DB);
        self.addr_manager().dump(
            OpenOptions::new()
                .write(true)
                .create(true)
                .append(false)
                .open(&tmp_addr_manager)?,
        )?;
        move_file(
            tmp_addr_manager,
            path.as_ref().join(DEFAULT_ADDR_MANAGER_DB),
        )?;
        self.ban_list().dump(
            OpenOptions::new()
                .write(true)
                .create(true)
                .append(false)
                .open(&tmp_ban_list)?,
        )?;
        move_file(tmp_ban_list, path.as_ref().join(DEFAULT_BAN_LIST_DB))?;
        Ok(())
    }
}

/// This function use `copy` then `remove_file` as a fallback when `rename` failed,
/// this maybe happen when src and dst on different file systems.
fn move_file<P: AsRef<Path>>(src: P, dst: P) -> Result<(), Error> {
    if rename(&src, &dst).is_err() {
        copy(&src, &dst)?;
        remove_file(&src)?;
    }
    Ok(())
}
