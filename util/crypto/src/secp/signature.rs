use super::error::Error;
use super::pubkey::Pubkey;
use super::Message;
use super::SECP256K1;
use faster_hex::hex_string;
use numext_fixed_hash::{H256, H520};
use secp256k1::schnorr::Signature as SchnorrSignature;
use secp256k1::Message as SecpMessage;
use secp256k1::{RecoverableSignature, RecoveryId};
use std::fmt;
use std::str::FromStr;

//RecoverableSignature compact serialize
#[derive(Clone)]
pub struct Signature([u8; 65]);

const HALF_N: H256 = H256([
    127, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 93, 87, 110,
    115, 87, 164, 80, 29, 223, 233, 47, 70, 104, 27, 32, 160,
]);
const N: H256 = H256([
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 254, 186, 174, 220,
    230, 175, 72, 160, 59, 191, 210, 94, 140, 208, 54, 65, 65,
]);

const ONE: H256 = H256([
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1,
]);

impl Signature {
    /// Get a slice into the 'r' portion of the data.
    pub fn r(&self) -> &[u8] {
        &self.0[0..32]
    }

    /// Get a slice into the 's' portion of the data.
    pub fn s(&self) -> &[u8] {
        &self.0[32..64]
    }

    /// Get the recovery id.
    pub fn v(&self) -> u8 {
        self.0[64]
    }

    pub fn from_compact(rec_id: RecoveryId, ret: [u8; 64]) -> Self {
        let mut data = [0; 65];
        data[0..64].copy_from_slice(&ret[0..64]);
        data[64] = rec_id.to_i32() as u8;
        Signature(data)
    }

    pub fn from_schnorr(sig: SchnorrSignature) -> Self {
        let mut data = [0; 65];
        data[0..64].copy_from_slice(&sig.serialize());
        data[64] = 0u8;
        Signature(data)
    }

    pub fn to_schnorr(&self) -> SchnorrSignature {
        SchnorrSignature::deserialize(&self.0[0..64])
    }

    /// Create a signature object from the sig.
    pub fn from_rsv(r: &H256, s: &H256, v: u8) -> Self {
        let mut sig = [0u8; 65];
        sig[0..32].copy_from_slice(r.as_bytes());
        sig[32..64].copy_from_slice(s.as_bytes());
        sig[64] = v;
        Signature(sig)
    }

    /// Check if this is a "low" signature.
    pub fn is_low_s(&self) -> bool {
        if let Ok(h_s) = H256::from_slice(self.s()) {
            h_s <= HALF_N
        } else {
            false
        }
    }

    /// Check if each component of the signature is in range.
    pub fn is_valid(&self) -> bool {
        let h_r = match H256::from_slice(self.r()) {
            Ok(h_r) => h_r,
            Err(_) => {
                return false;
            }
        };

        let h_s = match H256::from_slice(self.s()) {
            Ok(h_s) => h_s,
            Err(_) => {
                return false;
            }
        };
        self.v() <= 1 && h_r < N && h_r >= ONE && h_s < N && h_s >= ONE
    }

    /// Converts compact signature to a recoverable signature
    pub fn to_recoverable(&self) -> Result<RecoverableSignature, Error> {
        let context = &SECP256K1;
        let recovery_id = RecoveryId::from_i32(i32::from(self.0[64]))?;
        Ok(RecoverableSignature::from_compact(
            context,
            &self.0[0..64],
            recovery_id,
        )?)
    }

    /// Determines the public key for signature
    pub fn recover(&self, message: &Message) -> Result<Pubkey, Error> {
        let context = &SECP256K1;
        let recoverable_signature = self.to_recoverable()?;
        let message = SecpMessage::from_slice(message.as_bytes())?;
        let pubkey = context.recover(&message, &recoverable_signature)?;
        let serialized = pubkey.serialize_uncompressed();

        let mut pubkey = [0u8; 64];
        pubkey.copy_from_slice(&serialized[1..65]);
        Ok(pubkey.into())
    }

    pub fn recover_schnorr(&self, message: &Message) -> Result<Pubkey, Error> {
        let context = &SECP256K1;
        let schnorr_signature = self.to_schnorr();
        let message = SecpMessage::from_slice(&message[..])?;
        let pubkey = context.recover_schnorr(&message, &schnorr_signature)?;
        let serialized = pubkey.serialize_uncompressed();

        let mut pubkey = [0u8; 64];
        pubkey.copy_from_slice(&serialized[1..65]);
        Ok(pubkey.into())
    }

    pub fn serialize(&self) -> Vec<u8> {
        Vec::from(&self.0[..])
    }

    pub fn serialize_der(&self) -> Vec<u8> {
        let context = &SECP256K1;
        self.to_recoverable()
            .unwrap()
            .to_standard(context)
            .serialize_der(context)
    }
}

impl fmt::Debug for Signature {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        f.debug_struct("Signature")
            .field("r", &hex_string(&self.0[0..32]).expect("hex string"))
            .field("s", &hex_string(&self.0[32..64]).expect("hex string"))
            .field("v", &hex_string(&self.0[64..65]).expect("hex string"))
            .finish()
    }
}

impl From<H520> for Signature {
    fn from(sig: H520) -> Self {
        Signature(sig.0)
    }
}

impl From<Signature> for H520 {
    fn from(s: Signature) -> Self {
        H520(s.0)
    }
}

impl From<Vec<u8>> for Signature {
    fn from(sig: Vec<u8>) -> Self {
        let mut data = [0; 65];
        data[0..65].copy_from_slice(sig.as_slice());
        Signature(data)
    }
}

impl FromStr for Signature {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        H520::from_str(s)
            .map(Into::into)
            .map_err(|_| Error::InvalidSignature)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_n() {
        let half: H256 =
            H256::from_hex_str("7FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF5D576E7357A4501DDFE92F46681B20A0")
                .unwrap();
        let n: H256 =
            H256::from_hex_str("fffffffffffffffffffffffffffffffebaaedce6af48a03bbfd25e8cd0364141")
                .unwrap();
        assert_eq!(half, HALF_N);
        assert_eq!(n, N);
    }
}
