use bigint::{H256, H512, H520};
use error::Error;
use rustc_hex::ToHex;
use secp::Message;
use secp::SECP256K1;
use secp::pubkey::Pubkey;
use secp::secp256k1::{RecoverableSignature, RecoveryId};
use secp::secp256k1::Message as SecpMessage;
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

    /// Create a signature object from the sig.
    pub fn from_rsv(r: &H256, s: &H256, v: u8) -> Self {
        let mut sig = [0u8; 65];
        sig[0..32].copy_from_slice(r);
        sig[32..64].copy_from_slice(s);
        sig[64] = v;
        Signature(sig)
    }

    /// Check if this is a "low" signature.
    pub fn is_low_s(&self) -> bool {
        H256::from_slice(self.s()) <= HALF_N
    }

    /// Check if each component of the signature is in range.
    pub fn is_valid(&self) -> bool {
        self.v() <= 1 && H256::from_slice(self.r()) < N && H256::from_slice(self.r()) >= 1.into()
            && H256::from_slice(self.s()) < N && H256::from_slice(self.s()) >= 1.into()
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
        let message = SecpMessage::from_slice(&message[..])?;
        let pubkey = context.recover(&message, &recoverable_signature)?;
        let serialized = pubkey.serialize_uncompressed();

        let mut pubkey = H512::default();
        pubkey.copy_from_slice(&serialized[1..65]);
        Ok(pubkey.into())
    }
}

impl fmt::Debug for Signature {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        f.debug_struct("Signature")
            .field("r", &self.0[0..32].to_hex())
            .field("s", &self.0[32..64].to_hex())
            .field("v", &self.0[64..65].to_hex())
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
        let half: H256 = "7FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF5D576E7357A4501DDFE92F46681B20A0".into();
        let n: H256 = "fffffffffffffffffffffffffffffffebaaedce6af48a03bbfd25e8cd0364141".into();
        assert_eq!(half, HALF_N);
        assert_eq!(n, N);
    }
}
