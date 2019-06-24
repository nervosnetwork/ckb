mod error;
mod passphrase;
mod util;

use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use chrono::{Datelike, Timelike, Utc};
use ckb_crypto::secp::SECP256K1;
use ckb_hash::blake2b_256;
use faster_hex::hex_decode;
use numext_fixed_hash::{H160, H256};
use rand::Rng;
use uuid::Uuid;

pub use error::Error;
pub use passphrase::{CipherParams, Crypto, KdfParams, ScryptParams, ScryptType};

const KEYSTORE_VERSION: u32 = 3;

pub struct KeyStore {
    keys_dir: PathBuf,
    storage: PassphraseKeyStore,
    files: HashMap<H160, PathBuf>,
    unlocked_keys: HashMap<H160, TimedKey>,
}

impl KeyStore {
    pub fn from_dir(dir: PathBuf, scrypt_type: ScryptType) -> Result<KeyStore, Error> {
        let abs_dir = dir.canonicalize()?;
        let mut key_store = KeyStore {
            keys_dir: abs_dir.clone(),
            storage: PassphraseKeyStore {
                keys_dir_path: abs_dir,
                scrypt_type,
            },
            files: HashMap::default(),
            unlocked_keys: HashMap::default(),
        };
        key_store.refresh_dir()?;
        Ok(key_store)
    }

    pub fn new_account(&mut self, password: &[u8]) -> Result<H160, Error> {
        let privkey = MasterPrivKey::try_new(1024)?;
        let key = Key::new(privkey);
        let abs_path = self.storage.store_key(key.filename(), &key, password)?;
        let address = key.address().clone();
        self.files.insert(address.clone(), abs_path);
        Ok(address)
    }
    pub fn get_accounts(&mut self) -> &HashMap<H160, PathBuf> {
        &self.files
    }
    pub fn has_account(&mut self, address: &H160) -> bool {
        self.files.contains_key(address)
    }

    pub fn update(
        &mut self,
        address: &H160,
        password: &[u8],
        new_password: &[u8],
    ) -> Result<(), Error> {
        self.refresh_dir()?;
        let filepath = self.get_filepath(address)?;
        let key = self.storage.get_key(address, &filepath, password)?;
        self.storage
            .store_key(&filepath, &key, new_password)
            .map(|_| ())
    }
    pub fn delete(&mut self, address: &H160, password: &[u8]) -> Result<(), Error> {
        self.refresh_dir()?;
        let filepath = self.get_filepath(address)?;
        let _key = self.storage.get_key(address, &filepath, password)?;
        fs::remove_file(&filepath).map_err(Into::into)
    }

    pub fn lock(&mut self, address: &H160) -> bool {
        self.unlocked_keys.remove(address).is_some()
    }
    pub fn unlock(&mut self, address: &H160, password: &[u8]) -> Result<KeyTimeout, Error> {
        self.unlock_inner(address, password, None)
    }
    pub fn timed_unlock(
        &mut self,
        address: &H160,
        password: &[u8],
        keep: Duration,
    ) -> Result<KeyTimeout, Error> {
        self.unlock_inner(address, password, Some(keep))
    }
    pub fn get_lock_timeout(&self, address: &H160) -> Option<KeyTimeout> {
        self.unlocked_keys
            .get(address)
            .map(|timed_key| timed_key.timeout)
    }

    pub fn import(
        &mut self,
        data: &serde_json::Value,
        password: &[u8],
        new_password: &[u8],
    ) -> Result<H160, Error> {
        let key = Key::from_json(data, password)?;
        let filepath = self.storage.store_key(key.filename(), &key, new_password)?;
        self.files.insert(key.address().clone(), filepath);
        Ok(key.address().clone())
    }
    pub fn import_secp_key(
        &mut self,
        key: &secp256k1::SecretKey,
        password: &[u8],
    ) -> Result<H160, Error> {
        let key = Key::new(MasterPrivKey::from_secp_key(key));
        let filepath = self.storage.store_key(key.filename(), &key, password)?;
        self.files.insert(key.address().clone(), filepath);
        Ok(key.address().clone())
    }
    pub fn import_key(&mut self, key: &Key, password: &[u8]) -> Result<H160, Error> {
        let filepath = self.storage.store_key(key.filename(), key, password)?;
        self.files.insert(key.address().clone(), filepath);
        Ok(key.address().clone())
    }
    pub fn export(
        &self,
        address: &H160,
        password: &[u8],
        new_password: &[u8],
        scrypt_type: ScryptType,
    ) -> Result<serde_json::Value, Error> {
        let filepath = self.get_filepath(address)?;
        let key = self.storage.get_key(address, &filepath, password)?;
        Ok(key.to_json(new_password, scrypt_type))
    }

    pub fn sign(&mut self, address: &H160, hash: &H256) -> Result<secp256k1::Signature, Error> {
        Ok(self.get_timed_key(address)?.master_privkey().sign(hash))
    }
    pub fn sign_recoverable(
        &mut self,
        address: &H160,
        hash: &H256,
    ) -> Result<secp256k1::RecoverableSignature, Error> {
        Ok(self
            .get_timed_key(address)?
            .master_privkey()
            .sign_recoverable(hash))
    }
    pub fn sign_with_password(
        &self,
        address: &H160,
        hash: &H256,
        password: &[u8],
    ) -> Result<secp256k1::Signature, Error> {
        let filepath = self.get_filepath(address)?;
        let key = self.storage.get_key(address, &filepath, password)?;
        Ok(key.master_privkey.sign(hash))
    }
    pub fn sign_recoverable_with_password(
        &self,
        address: &H160,
        hash: &H256,
        password: &[u8],
    ) -> Result<secp256k1::RecoverableSignature, Error> {
        let filepath = self.get_filepath(address)?;
        let key = self.storage.get_key(address, &filepath, password)?;
        Ok(key.master_privkey.sign_recoverable(hash))
    }

    // NOTE: assume refresh keystore directory is not a hot action
    fn refresh_dir(&mut self) -> Result<(), Error> {
        let mut files = HashMap::default();
        for entry in fs::read_dir(&self.keys_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_file() {
                let filename = path.file_name().and_then(OsStr::to_str).unwrap();
                if let Some(address_hex) = filename.rsplitn(2, "--").next() {
                    let mut address_bin = [0u8; 20];
                    if hex_decode(address_hex.as_bytes(), &mut address_bin).is_ok() {
                        if let Ok(address) = H160::from_slice(&address_bin) {
                            files.insert(address, path.to_path_buf());
                        }
                    }
                }
            }
        }
        self.files = files;
        Ok(())
    }

    fn get_timed_key(&mut self, address: &H160) -> Result<&TimedKey, Error> {
        let is_expired = self
            .unlocked_keys
            .get(address)
            .ok_or_else(|| Error::AccountLocked(address.clone()))?
            .is_expired();
        if is_expired {
            self.unlocked_keys.remove(address);
            return Err(Error::AccountLocked(address.clone()));
        }

        let timed_key = self
            .unlocked_keys
            .get(address)
            .ok_or_else(|| Error::AccountLocked(address.clone()))?;
        Ok(timed_key)
    }

    fn get_filepath(&self, address: &H160) -> Result<PathBuf, Error> {
        self.files
            .get(address)
            .cloned()
            .ok_or_else(|| Error::AccountNotFound(address.clone()))
    }

    fn unlock_inner(
        &mut self,
        address: &H160,
        password: &[u8],
        keep: Option<Duration>,
    ) -> Result<KeyTimeout, Error> {
        let filepath = self.get_filepath(address)?;
        let key = self.storage.get_key(address, filepath, password)?;
        let entry = self.unlocked_keys.entry(address.clone());
        let value = match entry {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => entry.insert(TimedKey::new_timed(key, Duration::default())),
        };
        value.extend(keep);
        Ok(value.timeout)
    }
}

/// KeyStore protected by password
struct PassphraseKeyStore {
    keys_dir_path: PathBuf,
    scrypt_type: ScryptType,
}

impl PassphraseKeyStore {
    // Loads and decrypts the key from disk.
    fn get_key<P: AsRef<Path>>(
        &self,
        address: &H160,
        filename: P,
        password: &[u8],
    ) -> Result<Key, Error> {
        let filepath = self.join_path(filename);
        let mut file = fs::File::open(&filepath)?;
        let data = serde_json::from_reader(&mut file)
            .map_err(|err| Error::ParseJsonFailed(err.to_string()))?;
        let key = Key::from_json(&data, password)?;
        if key.address() != address {
            return Err(Error::KeyMismatch {
                got: key.address().clone(),
                expected: address.clone(),
            });
        }
        Ok(key)
    }

    // Writes and encrypts the key.
    fn store_key<P: AsRef<Path>>(
        &self,
        filename: P,
        key: &Key,
        password: &[u8],
    ) -> Result<PathBuf, Error> {
        let filepath = self.join_path(filename);
        let json_value = key.to_json(password, self.scrypt_type);
        let mut file = fs::File::create(&filepath)?;
        serde_json::to_writer(&mut file, &json_value).map_err(|err| Error::Io(err.to_string()))?;
        Ok(filepath)
    }

    // Joins filename with the key directory unless it is already absolute.
    fn join_path<P: AsRef<Path>>(&self, filename: P) -> PathBuf {
        if filename.as_ref().is_absolute() {
            filename.as_ref().to_path_buf()
        } else {
            self.keys_dir_path.join(filename.as_ref())
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum KeyTimeout {
    Infinite,
    Timeout(Instant),
}

impl fmt::Display for KeyTimeout {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let output = match self {
            KeyTimeout::Timeout(timeout) => {
                let total_secs = (*timeout - Instant::now()).as_secs();
                let hours = total_secs / 3600;
                let left = total_secs % 3600;
                let minutes = left / 60;
                let seconds = left % 60;
                match (hours, minutes, seconds) {
                    (0, 0, seconds) => format!("{} seconds", seconds),
                    (0, minutes, seconds) => format!("{} minutes, {} seconds", minutes, seconds),
                    (hours, minutes, seconds) => {
                        format!("{} hours, {} minutes, {} seconds", hours, minutes, seconds,)
                    }
                }
            }
            KeyTimeout::Infinite => "infinite time".to_owned(),
        };
        write!(f, "{}", output)
    }
}

struct TimedKey {
    key: Key,
    timeout: KeyTimeout,
}

impl TimedKey {
    fn master_privkey(&self) -> &MasterPrivKey {
        &self.key.master_privkey
    }

    fn new_timed(key: Key, keep: Duration) -> TimedKey {
        let timeout = Instant::now() + keep;
        TimedKey {
            key,
            timeout: KeyTimeout::Timeout(timeout),
        }
    }

    fn extend(&mut self, extra: Option<Duration>) {
        if self.is_expired() {
            self.timeout = KeyTimeout::Timeout(Instant::now());
        }
        if let Some(extra) = extra {
            if let KeyTimeout::Timeout(ref mut timeout) = self.timeout {
                *timeout += extra;
            }
        } else {
            self.timeout = KeyTimeout::Infinite;
        }
    }

    fn is_expired(&self) -> bool {
        match self.timeout {
            KeyTimeout::Timeout(timeout) => timeout <= Instant::now(),
            KeyTimeout::Infinite => false,
        }
    }
}

pub struct Key {
    // randomly generate uuid v4
    id: Uuid,
    // H160::from_slice(&blake2b_256(pubkey)[0..20])
    address: H160,
    // The extended secp256k1 private key (privkey + chaincode)
    master_privkey: MasterPrivKey,
}

impl Key {
    pub fn new(master_privkey: MasterPrivKey) -> Key {
        let id = Uuid::new_v4();
        let address = master_privkey.address();
        Key {
            id,
            address,
            master_privkey,
        }
    }

    pub fn address(&self) -> &H160 {
        &self.address
    }

    pub fn filename(&self) -> String {
        let utc_now = Utc::now();
        let date = utc_now.date();
        let time = utc_now.time();
        format!(
            "UTC--{:04}-{:02}-{:02}T{:02}-{:02}-{:02}.{:09}Z--{:x}",
            date.year(),
            date.month(),
            date.day(),
            time.hour(),
            time.minute(),
            time.second(),
            time.nanosecond(),
            self.address(),
        )
    }

    pub fn from_json(data: &serde_json::Value, password: &[u8]) -> Result<Key, Error> {
        let id = util::get_str(data, "id").and_then(|id_str| {
            Uuid::parse_str(id_str)
                .map_err(|_| Error::ParseJsonFailed(format!("Invalid id: {}", id_str)))
        })?;

        let version = util::get_u64(data, "version")? as u32;
        if version != KEYSTORE_VERSION {
            return Err(Error::ParseJsonFailed(format!(
                "Unsupported keystore version: {}",
                version
            )));
        }

        let crypto = util::get_value(data, "crypto").and_then(|value| Crypto::from_json(value))?;
        if crypto.ciphertext().len() != 64 {
            return Err(Error::ParseJsonFailed(format!(
                "Invalid ciphertext length: {}, expected: 64",
                crypto.ciphertext().len()
            )));
        }
        let key_vec = crypto.decrypt(password)?;
        let mut key_bytes = [0u8; 64];
        key_bytes[..].copy_from_slice(&key_vec[..]);
        let master_privkey = MasterPrivKey::from_bytes(key_bytes)?;

        let address = master_privkey.address();
        Ok(Key {
            id,
            address,
            master_privkey,
        })
    }

    pub fn to_json(&self, password: &[u8], scrypt_type: ScryptType) -> serde_json::Value {
        let mut buf = Uuid::encode_buffer();
        let id_str = self.id.to_hyphenated().encode_lower(&mut buf);
        let address_hex = format!("{:x}", self.address);
        let master_privkey = self.master_privkey.to_bytes();
        let crypto = Crypto::encrypt_key_scrypt(&master_privkey, password, scrypt_type);
        serde_json::json!({
            "id": id_str,
            "version": KEYSTORE_VERSION,
            "address": address_hex,
            "crypto": crypto.to_json(),
        })
    }
}

pub struct MasterPrivKey {
    secp_secret_key: secp256k1::SecretKey,
    chain_code: [u8; 32],
}

impl MasterPrivKey {
    pub fn try_new(time: u16) -> Result<MasterPrivKey, Error> {
        let mut rng = rand::thread_rng();
        for _ in 0..time {
            let privkey_bytes: [u8; 32] = rng.gen();
            if let Ok(secp_secret_key) = secp256k1::SecretKey::from_slice(&privkey_bytes) {
                let chain_code: [u8; 32] = rng.gen();
                return Ok(MasterPrivKey {
                    secp_secret_key,
                    chain_code,
                });
            }
        }
        Err(Error::GenSecpFailed(time))
    }

    pub fn from_secp_key(secp_secret_key: &secp256k1::SecretKey) -> MasterPrivKey {
        let secp_secret_key = *secp_secret_key;
        let mut rng = rand::thread_rng();
        let chain_code = rng.gen();
        MasterPrivKey {
            secp_secret_key,
            chain_code,
        }
    }

    pub fn from_bytes(bytes: [u8; 64]) -> Result<MasterPrivKey, Error> {
        let secp_secret_key = secp256k1::SecretKey::from_slice(&bytes[0..32])
            .map_err(|_| Error::InvalidSecpSecret)?;
        let mut chain_code_bytes = [0u8; 32];
        chain_code_bytes.copy_from_slice(&bytes[32..64]);
        Ok(MasterPrivKey {
            secp_secret_key,
            chain_code: chain_code_bytes,
        })
    }

    pub fn to_bytes(&self) -> [u8; 64] {
        let mut bytes = [0u8; 64];
        bytes[0..32].copy_from_slice(&self.secp_secret_key[..]);
        bytes[32..64].copy_from_slice(&self.chain_code[..]);
        bytes
    }

    pub fn sign(&self, hash: &H256) -> secp256k1::Signature {
        let message = secp256k1::Message::from_slice(&hash[..]).expect("Convert to message failed");
        SECP256K1.sign(&message, &self.secp_secret_key)
    }

    pub fn sign_recoverable(&self, hash: &H256) -> secp256k1::RecoverableSignature {
        let message = secp256k1::Message::from_slice(&hash[..]).expect("Convert to message failed");
        SECP256K1.sign_recoverable(&message, &self.secp_secret_key)
    }

    pub fn address(&self) -> H160 {
        let pubkey = secp256k1::PublicKey::from_secret_key(&SECP256K1, &self.secp_secret_key);
        H160::from_slice(&blake2b_256(&pubkey.serialize()[..])[0..20])
            .expect("Generate hash(H160) from pubkey failed")
    }
}
