// Rust Bitcoin Library
// Written in 2014 by
//     Andrew Poelstra <apoelstra@wpsoftware.net>
// To the extent possible under law, the author(s) have dedicated all
// copyright and related and neighboring rights to this software to
// the public domain worldwide. This software is distributed without
// any warranty.
//
// You should have received a copy of the CC0 Public Domain Dedication
// along with this software.
// If not, see <http://creativecommons.org/publicdomain/zero/1.0/>.
//

//! BIP32 Implementation
//!
//! Implementation of BIP32 hierarchical deterministic wallets, as defined
//! at https://github.com/bitcoin/bips/blob/master/bip-0032.mediawiki

use std::default::Default;
use std::io::Write;
use std::str::FromStr;
use std::{error, fmt};

use bitcoin_hashes::{hash160, sha512, Hash, HashEngine, Hmac, HmacEngine};
use byteorder::{BigEndian, ByteOrder};
use secp256k1::{self, PublicKey, Secp256k1, SecretKey};

use crate::NetworkType;

macro_rules! impl_array_newtype {
    ($thing:ident, $ty:ty, $len:expr) => {
        impl<'a> From<&'a [$ty]> for $thing {
            fn from(data: &'a [$ty]) -> $thing {
                assert_eq!(data.len(), $len);
                let mut ret = [0; $len];
                ret.copy_from_slice(&data[..]);
                $thing(ret)
            }
        }

        impl ::std::ops::Index<usize> for $thing {
            type Output = $ty;

            #[inline]
            fn index(&self, index: usize) -> &$ty {
                let &$thing(ref dat) = self;
                &dat[index]
            }
        }

        impl_index_newtype!($thing, $ty);

        impl PartialEq for $thing {
            #[inline]
            fn eq(&self, other: &$thing) -> bool {
                &self[..] == &other[..]
            }
        }

        impl Eq for $thing {}

        impl PartialOrd for $thing {
            #[inline]
            fn partial_cmp(&self, other: &$thing) -> Option<::std::cmp::Ordering> {
                Some(self.cmp(&other))
            }
        }

        impl Ord for $thing {
            #[inline]
            fn cmp(&self, other: &$thing) -> ::std::cmp::Ordering {
                // manually implement comparison to get little-endian ordering
                // (we need this for our numeric types; non-numeric ones shouldn't
                // be ordered anyway except to put them in BTrees or whatever, and
                // they don't care how we order as long as we're consistent).
                for i in 0..$len {
                    if self[$len - 1 - i] < other[$len - 1 - i] {
                        return ::std::cmp::Ordering::Less;
                    }
                    if self[$len - 1 - i] > other[$len - 1 - i] {
                        return ::std::cmp::Ordering::Greater;
                    }
                }
                ::std::cmp::Ordering::Equal
            }
        }

        impl ::std::hash::Hash for $thing {
            #[inline]
            fn hash<H>(&self, state: &mut H)
            where
                H: ::std::hash::Hasher,
            {
                (&self[..]).hash(state);
            }

            fn hash_slice<H>(data: &[$thing], state: &mut H)
            where
                H: ::std::hash::Hasher,
            {
                for d in data.iter() {
                    (&d[..]).hash(state);
                }
            }
        }
    };
}

macro_rules! impl_index_newtype {
    ($thing:ident, $ty:ty) => {
        impl ::std::ops::Index<::std::ops::Range<usize>> for $thing {
            type Output = [$ty];

            #[inline]
            fn index(&self, index: ::std::ops::Range<usize>) -> &[$ty] {
                &self.0[index]
            }
        }

        impl ::std::ops::Index<::std::ops::RangeTo<usize>> for $thing {
            type Output = [$ty];

            #[inline]
            fn index(&self, index: ::std::ops::RangeTo<usize>) -> &[$ty] {
                &self.0[index]
            }
        }

        impl ::std::ops::Index<::std::ops::RangeFrom<usize>> for $thing {
            type Output = [$ty];

            #[inline]
            fn index(&self, index: ::std::ops::RangeFrom<usize>) -> &[$ty] {
                &self.0[index]
            }
        }

        impl ::std::ops::Index<::std::ops::RangeFull> for $thing {
            type Output = [$ty];

            #[inline]
            fn index(&self, _: ::std::ops::RangeFull) -> &[$ty] {
                &self.0[..]
            }
        }
    };
}

/// A chain code
#[derive(Debug, Clone, Copy)]
pub struct ChainCode(pub [u8; 32]);
impl_array_newtype!(ChainCode, u8, 32);

/// A fingerprint
#[derive(Debug, Clone, Copy)]
pub struct Fingerprint(pub [u8; 4]);
impl_array_newtype!(Fingerprint, u8, 4);

impl Default for Fingerprint {
    fn default() -> Fingerprint {
        Fingerprint([0; 4])
    }
}

/// Extended private key
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ExtendedPrivKey {
    /// The network this key is to be used on
    pub network: NetworkType,
    /// How many derivations this key is from the master (which is 0)
    pub depth: u8,
    /// Fingerprint of the parent key (0 for master)
    pub parent_fingerprint: Fingerprint,
    /// Child number of the key used to derive from parent (0 for master)
    pub child_number: ChildNumber,
    /// Private key
    pub private_key: SecretKey,
    /// Chain code
    pub chain_code: ChainCode,
}

/// Extended public key
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ExtendedPubKey {
    /// The network this key is to be used on
    pub network: NetworkType,
    /// How many derivations this key is from the master (which is 0)
    pub depth: u8,
    /// Fingerprint of the parent key
    pub parent_fingerprint: Fingerprint,
    /// Child number of the key used to derive from parent (0 for master)
    pub child_number: ChildNumber,
    /// Public key
    pub public_key: PublicKey,
    /// Chain code
    pub chain_code: ChainCode,
}

/// A child number for a derived key
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum ChildNumber {
    /// Non-hardened key
    Normal {
        /// Key index, within [0, 2^31 - 1]
        index: u32,
    },
    /// Hardened key
    Hardened {
        /// Key index, within [0, 2^31 - 1]
        index: u32,
    },
}

impl ChildNumber {
    /// Create a [`Normal`] from an index, returns an error if the index is not within
    /// [0, 2^31 - 1].
    ///
    /// [`Normal`]: #variant.Normal
    pub fn from_normal_idx(index: u32) -> Result<Self, Error> {
        if index & (1 << 31) == 0 {
            Ok(ChildNumber::Normal { index })
        } else {
            Err(Error::InvalidChildNumber(index))
        }
    }

    /// Create a [`Hardened`] from an index, returns an error if the index is not within
    /// [0, 2^31 - 1].
    ///
    /// [`Hardened`]: #variant.Hardened
    pub fn from_hardened_idx(index: u32) -> Result<Self, Error> {
        if index & (1 << 31) == 0 {
            Ok(ChildNumber::Hardened { index })
        } else {
            Err(Error::InvalidChildNumber(index))
        }
    }

    /// Returns `true` if the child number is a [`Normal`] value.
    ///
    /// [`Normal`]: #variant.Normal
    pub fn is_normal(self) -> bool {
        !self.is_hardened()
    }

    /// Returns `true` if the child number is a [`Hardened`] value.
    ///
    /// [`Hardened`]: #variant.Hardened
    pub fn is_hardened(self) -> bool {
        match self {
            ChildNumber::Hardened { .. } => true,
            ChildNumber::Normal { .. } => false,
        }
    }

    /// Returns the child number that is a single increment from this one.
    pub fn increment(self) -> Result<ChildNumber, Error> {
        match self {
            ChildNumber::Normal { index: idx } => ChildNumber::from_normal_idx(idx + 1),
            ChildNumber::Hardened { index: idx } => ChildNumber::from_hardened_idx(idx + 1),
        }
    }
}

impl From<u32> for ChildNumber {
    fn from(number: u32) -> Self {
        if number & (1 << 31) != 0 {
            ChildNumber::Hardened {
                index: number ^ (1 << 31),
            }
        } else {
            ChildNumber::Normal { index: number }
        }
    }
}

impl From<ChildNumber> for u32 {
    fn from(cnum: ChildNumber) -> Self {
        match cnum {
            ChildNumber::Normal { index } => index,
            ChildNumber::Hardened { index } => index | (1 << 31),
        }
    }
}

impl fmt::Display for ChildNumber {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            ChildNumber::Hardened { index } => write!(f, "{}'", index),
            ChildNumber::Normal { index } => write!(f, "{}", index),
        }
    }
}

impl FromStr for ChildNumber {
    type Err = Error;

    fn from_str(inp: &str) -> Result<ChildNumber, Error> {
        let child_number = if inp.chars().last().map_or(false, |l| l == '\'' || l == 'h') {
            ChildNumber::from_hardened_idx(
                inp[0..inp.len() - 1]
                    .parse()
                    .map_err(|_| Error::InvalidChildNumberFormat)?,
            )?
        } else {
            ChildNumber::from_normal_idx(inp.parse().map_err(|_| Error::InvalidChildNumberFormat)?)?
        };
        Ok(child_number)
    }
}

/// A BIP-32 derivation path.
#[derive(Clone, PartialEq, Eq)]
pub struct DerivationPath(Vec<ChildNumber>);
impl_index_newtype!(DerivationPath, ChildNumber);

impl From<Vec<ChildNumber>> for DerivationPath {
    fn from(numbers: Vec<ChildNumber>) -> Self {
        DerivationPath(numbers)
    }
}

impl Into<Vec<ChildNumber>> for DerivationPath {
    fn into(self) -> Vec<ChildNumber> {
        self.0
    }
}

impl<'a> From<&'a [ChildNumber]> for DerivationPath {
    fn from(numbers: &'a [ChildNumber]) -> Self {
        DerivationPath(numbers.to_vec())
    }
}

impl ::std::iter::FromIterator<ChildNumber> for DerivationPath {
    fn from_iter<T>(iter: T) -> Self
    where
        T: IntoIterator<Item = ChildNumber>,
    {
        DerivationPath(Vec::from_iter(iter))
    }
}

impl<'a> ::std::iter::IntoIterator for &'a DerivationPath {
    type Item = &'a ChildNumber;
    type IntoIter = ::std::slice::Iter<'a, ChildNumber>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

impl AsRef<[ChildNumber]> for DerivationPath {
    fn as_ref(&self) -> &[ChildNumber] {
        &self.0
    }
}

impl FromStr for DerivationPath {
    type Err = Error;

    fn from_str(path: &str) -> Result<DerivationPath, Error> {
        let mut parts = path.split('/');
        // First parts must be `m`.
        if parts.next().unwrap() != "m" {
            return Err(Error::InvalidDerivationPathFormat);
        }

        let ret: Result<Vec<ChildNumber>, Error> = parts.map(str::parse).collect();
        Ok(DerivationPath(ret?))
    }
}

/// An iterator over children of a [DerivationPath].
///
/// It is returned by the methods [DerivationPath::children_since],
/// [DerivationPath::normal_children] and [DerivationPath::hardened_children].
pub struct DerivationPathIterator<'a> {
    base: &'a DerivationPath,
    next_child: Option<ChildNumber>,
}

impl<'a> DerivationPathIterator<'a> {
    /// Start a new [DerivationPathIterator] at the given child.
    pub fn start_from(path: &'a DerivationPath, start: ChildNumber) -> DerivationPathIterator<'a> {
        DerivationPathIterator {
            base: path,
            next_child: Some(start),
        }
    }
}

impl<'a> Iterator for DerivationPathIterator<'a> {
    type Item = DerivationPath;

    fn next(&mut self) -> Option<Self::Item> {
        let ret = self.next_child?;
        self.next_child = ret.increment().ok();
        Some(self.base.child(ret))
    }
}

impl DerivationPath {
    /// Create a new [DerivationPath] that is a child of this one.
    pub fn child(&self, cn: ChildNumber) -> DerivationPath {
        let mut path = self.0.clone();
        path.push(cn);
        DerivationPath(path)
    }

    /// Convert into a [DerivationPath] that is a child of this one.
    pub fn into_child(self, cn: ChildNumber) -> DerivationPath {
        let mut path = self.0;
        path.push(cn);
        DerivationPath(path)
    }

    /// Get an [Iterator] over the children of this [DerivationPath]
    /// starting with the given [ChildNumber].
    pub fn children_from(&self, cn: ChildNumber) -> DerivationPathIterator {
        DerivationPathIterator::start_from(&self, cn)
    }

    /// Get an [Iterator] over the unhardened children of this [DerivationPath].
    pub fn normal_children(&self) -> DerivationPathIterator {
        DerivationPathIterator::start_from(&self, ChildNumber::Normal { index: 0 })
    }

    /// Get an [Iterator] over the hardened children of this [DerivationPath].
    pub fn hardened_children(&self) -> DerivationPathIterator {
        DerivationPathIterator::start_from(&self, ChildNumber::Hardened { index: 0 })
    }
}

impl fmt::Display for DerivationPath {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("m")?;
        for cn in self.0.iter() {
            f.write_str("/")?;
            fmt::Display::fmt(cn, f)?;
        }
        Ok(())
    }
}

impl fmt::Debug for DerivationPath {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(&self, f)
    }
}

/// A BIP32 error
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Error {
    /// A pk->pk derivation was attempted on a hardened key
    CannotDeriveFromHardenedKey,
    /// A secp256k1 error occurred
    Ecdsa(secp256k1::Error),
    /// A child number was provided that was out of range
    InvalidChildNumber(u32),
    /// Error creating a master seed --- for application use
    RngError(String),
    /// Invalid childnumber format.
    InvalidChildNumberFormat,
    /// Invalid derivation path format.
    InvalidDerivationPathFormat,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            Error::CannotDeriveFromHardenedKey => {
                f.write_str("cannot derive hardened key from public key")
            }
            Error::Ecdsa(ref e) => fmt::Display::fmt(e, f),
            Error::InvalidChildNumber(ref n) => write!(
                f,
                "child number {} is invalid (not within [0, 2^31 - 1])",
                n
            ),
            Error::RngError(ref s) => write!(f, "rng error {}", s),
            Error::InvalidChildNumberFormat => f.write_str("invalid child number format"),
            Error::InvalidDerivationPathFormat => f.write_str("invalid derivation path format"),
        }
    }
}

impl error::Error for Error {
    fn cause(&self) -> Option<&error::Error> {
        if let Error::Ecdsa(ref e) = *self {
            Some(e)
        } else {
            None
        }
    }

    fn description(&self) -> &str {
        match *self {
            Error::CannotDeriveFromHardenedKey => "cannot derive hardened key from public key",
            Error::Ecdsa(ref e) => error::Error::description(e),
            Error::InvalidChildNumber(_) => "child number is invalid",
            Error::RngError(_) => "rng error",
            Error::InvalidChildNumberFormat => "invalid child number format",
            Error::InvalidDerivationPathFormat => "invalid derivation path format",
        }
    }
}

impl From<secp256k1::Error> for Error {
    fn from(e: secp256k1::Error) -> Error {
        Error::Ecdsa(e)
    }
}

impl ExtendedPrivKey {
    /// Construct a new master key from a seed value
    pub fn new_master(network: NetworkType, seed: &[u8]) -> Result<ExtendedPrivKey, Error> {
        let mut hmac_engine: HmacEngine<sha512::Hash> = HmacEngine::new(b"Bitcoin seed");
        hmac_engine.input(seed);
        let hmac_result: Hmac<sha512::Hash> = Hmac::from_engine(hmac_engine);

        Ok(ExtendedPrivKey {
            network,
            depth: 0,
            parent_fingerprint: Default::default(),
            child_number: ChildNumber::from_normal_idx(0)?,
            private_key: secp256k1::SecretKey::from_slice(&hmac_result[..32])
                .map_err(Error::Ecdsa)?,
            chain_code: ChainCode::from(&hmac_result[32..]),
        })
    }

    /// Attempts to derive an extended private key from a path.
    ///
    /// The `path` argument can be both of type `DerivationPath` or `Vec<ChildNumber>`.
    pub fn derive_priv<C: secp256k1::Signing, P: AsRef<[ChildNumber]>>(
        &self,
        secp: &Secp256k1<C>,
        path: &P,
    ) -> Result<ExtendedPrivKey, Error> {
        let mut sk: ExtendedPrivKey = *self;
        for cnum in path.as_ref() {
            sk = sk.ckd_priv(secp, *cnum)?;
        }
        Ok(sk)
    }

    /// Private->Private child key derivation
    pub fn ckd_priv<C: secp256k1::Signing>(
        &self,
        secp: &Secp256k1<C>,
        i: ChildNumber,
    ) -> Result<ExtendedPrivKey, Error> {
        let mut hmac_engine: HmacEngine<sha512::Hash> = HmacEngine::new(&self.chain_code[..]);
        let mut be_n = [0; 4];
        match i {
            ChildNumber::Normal { .. } => {
                // Non-hardened key: compute public data and use that
                hmac_engine
                    .input(&PublicKey::from_secret_key(secp, &self.private_key).serialize()[..]);
            }
            ChildNumber::Hardened { .. } => {
                // Hardened key: use only secret data to prevent public derivation
                hmac_engine.input(&[0u8]);
                hmac_engine.input(&self.private_key[..]);
            }
        }
        BigEndian::write_u32(&mut be_n, u32::from(i));

        hmac_engine.input(&be_n);
        let hmac_result: Hmac<sha512::Hash> = Hmac::from_engine(hmac_engine);
        let mut sk = secp256k1::SecretKey::from_slice(&hmac_result[..32]).map_err(Error::Ecdsa)?;
        sk.add_assign(&self.private_key[..]).map_err(Error::Ecdsa)?;

        Ok(ExtendedPrivKey {
            network: self.network,
            depth: self.depth + 1,
            parent_fingerprint: self.fingerprint(secp),
            child_number: i,
            private_key: sk,
            chain_code: ChainCode::from(&hmac_result[32..]),
        })
    }

    /// Returns the HASH160 of the chaincode
    pub fn identifier<C: secp256k1::Signing>(&self, secp: &Secp256k1<C>) -> hash160::Hash {
        ExtendedPubKey::from_private(secp, self).identifier()
    }

    /// Returns the first four bytes of the identifier
    pub fn fingerprint<C: secp256k1::Signing>(&self, secp: &Secp256k1<C>) -> Fingerprint {
        Fingerprint::from(&self.identifier(secp)[0..4])
    }
}

impl ExtendedPubKey {
    /// Derives a public key from a private key
    pub fn from_private<C: secp256k1::Signing>(
        secp: &Secp256k1<C>,
        sk: &ExtendedPrivKey,
    ) -> ExtendedPubKey {
        ExtendedPubKey {
            network: sk.network,
            depth: sk.depth,
            parent_fingerprint: sk.parent_fingerprint,
            child_number: sk.child_number,
            public_key: secp256k1::PublicKey::from_secret_key(secp, &sk.private_key),
            chain_code: sk.chain_code,
        }
    }

    /// Attempts to derive an extended public key from a path.
    ///
    /// The `path` argument can be both of type `DerivationPath` or `Vec<ChildNumber>`.
    pub fn derive_pub<C: secp256k1::Verification, P: AsRef<[ChildNumber]>>(
        &self,
        secp: &Secp256k1<C>,
        path: &P,
    ) -> Result<ExtendedPubKey, Error> {
        let mut pk: ExtendedPubKey = *self;
        for cnum in path.as_ref() {
            pk = pk.ckd_pub(secp, *cnum)?
        }
        Ok(pk)
    }

    /// Compute the scalar tweak added to this key to get a child key
    pub fn ckd_pub_tweak(&self, i: ChildNumber) -> Result<(SecretKey, ChainCode), Error> {
        match i {
            ChildNumber::Hardened { .. } => Err(Error::CannotDeriveFromHardenedKey),
            ChildNumber::Normal { index: n } => {
                let mut hmac_engine: HmacEngine<sha512::Hash> =
                    HmacEngine::new(&self.chain_code[..]);
                hmac_engine.input(&self.public_key.serialize()[..]);
                let mut be_n = [0; 4];
                BigEndian::write_u32(&mut be_n, n);
                hmac_engine.input(&be_n);

                let hmac_result: Hmac<sha512::Hash> = Hmac::from_engine(hmac_engine);

                let private_key = secp256k1::SecretKey::from_slice(&hmac_result[..32])?;
                let chain_code = ChainCode::from(&hmac_result[32..]);
                Ok((private_key, chain_code))
            }
        }
    }

    /// Public->Public child key derivation
    pub fn ckd_pub<C: secp256k1::Verification>(
        &self,
        secp: &Secp256k1<C>,
        i: ChildNumber,
    ) -> Result<ExtendedPubKey, Error> {
        let (sk, chain_code) = self.ckd_pub_tweak(i)?;
        let mut pk = self.public_key;
        pk.add_exp_assign(secp, &sk[..]).map_err(Error::Ecdsa)?;

        Ok(ExtendedPubKey {
            network: self.network,
            depth: self.depth + 1,
            parent_fingerprint: self.fingerprint(),
            child_number: i,
            public_key: pk,
            chain_code,
        })
    }

    /// Returns the HASH160 of the chaincode
    pub fn identifier(&self) -> hash160::Hash {
        let mut engine = hash160::Hash::engine();
        engine
            .write_all(&self.public_key.serialize())
            .expect("write must ok");
        hash160::Hash::from_engine(engine)
    }

    /// Returns the first four bytes of the identifier
    pub fn fingerprint(&self) -> Fingerprint {
        Fingerprint::from(&self.identifier()[0..4])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use bitcoin_hashes::{sha256d, Hash};
    use byteorder::{LittleEndian, ReadBytesExt};
    use faster_hex::hex_decode;
    use std::io::Cursor;
    use std::{error, fmt, iter, slice, str};

    /// An error that might occur during base58 decoding
    #[derive(Debug, PartialEq, Eq, Clone)]
    pub enum B58Error {
        /// Invalid character encountered
        BadByte(u8),
        /// Checksum was not correct (expected, actual)
        BadChecksum(u32, u32),
        /// The length (in bytes) of the object was not correct
        /// Note that if the length is excessively long the provided length may be
        /// an estimate (and the checksum step may be skipped).
        InvalidLength(usize),
        /// Version byte(s) were not recognized
        InvalidVersion(Vec<u8>),
        /// Checked data was less than 4 bytes
        TooShort(usize),
        /// Any other error
        Other(String),
    }

    impl fmt::Display for B58Error {
        fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
            match *self {
                B58Error::BadByte(b) => write!(f, "invalid base58 character 0x{:x}", b),
                B58Error::BadChecksum(exp, actual) => write!(
                    f,
                    "base58ck checksum 0x{:x} does not match expected 0x{:x}",
                    actual, exp
                ),
                B58Error::InvalidLength(ell) => {
                    write!(f, "length {} invalid for this base58 type", ell)
                }
                B58Error::InvalidVersion(ref v) => {
                    write!(f, "version {:?} invalid for this base58 type", v)
                }
                B58Error::TooShort(_) => {
                    write!(f, "base58ck data not even long enough for a checksum")
                }
                B58Error::Other(ref s) => f.write_str(s),
            }
        }
    }

    impl error::Error for B58Error {
        fn cause(&self) -> Option<&error::Error> {
            None
        }
        fn description(&self) -> &'static str {
            match *self {
                B58Error::BadByte(_) => "invalid b58 character",
                B58Error::BadChecksum(_, _) => "invalid b58ck checksum",
                B58Error::InvalidLength(_) => "invalid length for b58 type",
                B58Error::InvalidVersion(_) => "invalid version for b58 type",
                B58Error::TooShort(_) => "b58ck data less than 4 bytes",
                B58Error::Other(_) => "unknown b58 error",
            }
        }
    }

    /// Vector-like object that holds the first 100 elements on the stack. If more space is needed it
    /// will be allocated on the heap.
    struct SmallVec<T> {
        len: usize,
        stack: [T; 100],
        heap: Vec<T>,
    }

    impl<T: Default + Copy> SmallVec<T> {
        pub fn new() -> SmallVec<T> {
            SmallVec {
                len: 0,
                stack: [T::default(); 100],
                heap: Vec::new(),
            }
        }

        pub fn push(&mut self, val: T) {
            if self.len < 100 {
                self.stack[self.len] = val;
                self.len += 1;
            } else {
                self.heap.push(val);
            }
        }

        pub fn iter(&self) -> iter::Chain<slice::Iter<T>, slice::Iter<T>> {
            // If len<100 then we just append an empty vec
            self.stack[0..self.len].iter().chain(self.heap.iter())
        }

        pub fn iter_mut(&mut self) -> iter::Chain<slice::IterMut<T>, slice::IterMut<T>> {
            // If len<100 then we just append an empty vec
            self.stack[0..self.len]
                .iter_mut()
                .chain(self.heap.iter_mut())
        }
    }

    static BASE58_CHARS: &'static [u8] =
        b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";

    static BASE58_DIGITS: [Option<u8>; 128] = [
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None, // 0-7
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None, // 8-15
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None, // 16-23
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None, // 24-31
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None, // 32-39
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None, // 40-47
        None,
        Some(0),
        Some(1),
        Some(2),
        Some(3),
        Some(4),
        Some(5),
        Some(6), // 48-55
        Some(7),
        Some(8),
        None,
        None,
        None,
        None,
        None,
        None, // 56-63
        None,
        Some(9),
        Some(10),
        Some(11),
        Some(12),
        Some(13),
        Some(14),
        Some(15), // 64-71
        Some(16),
        None,
        Some(17),
        Some(18),
        Some(19),
        Some(20),
        Some(21),
        None, // 72-79
        Some(22),
        Some(23),
        Some(24),
        Some(25),
        Some(26),
        Some(27),
        Some(28),
        Some(29), // 80-87
        Some(30),
        Some(31),
        Some(32),
        None,
        None,
        None,
        None,
        None, // 88-95
        None,
        Some(33),
        Some(34),
        Some(35),
        Some(36),
        Some(37),
        Some(38),
        Some(39), // 96-103
        Some(40),
        Some(41),
        Some(42),
        Some(43),
        None,
        Some(44),
        Some(45),
        Some(46), // 104-111
        Some(47),
        Some(48),
        Some(49),
        Some(50),
        Some(51),
        Some(52),
        Some(53),
        Some(54), // 112-119
        Some(55),
        Some(56),
        Some(57),
        None,
        None,
        None,
        None,
        None, // 120-127
    ];

    /// Decode base58-encoded string into a byte vector
    fn from(data: &str) -> Result<Vec<u8>, B58Error> {
        // 11/15 is just over log_256(58)
        let mut scratch = vec![0u8; 1 + data.len() * 11 / 15];
        // Build in base 256
        for d58 in data.bytes() {
            // Compute "X = X * 58 + next_digit" in base 256
            if d58 as usize > BASE58_DIGITS.len() {
                return Err(B58Error::BadByte(d58));
            }
            let mut carry = match BASE58_DIGITS[d58 as usize] {
                Some(d58) => u32::from(d58),
                None => {
                    return Err(B58Error::BadByte(d58));
                }
            };
            for d256 in scratch.iter_mut().rev() {
                carry += u32::from(*d256) * 58;
                *d256 = carry as u8;
                carry /= 256;
            }
            assert_eq!(carry, 0);
        }

        // Copy leading zeroes directly
        let mut ret: Vec<u8> = data
            .bytes()
            .take_while(|&x| x == BASE58_CHARS[0])
            .map(|_| 0)
            .collect();
        // Copy rest of string
        ret.extend(scratch.into_iter().skip_while(|&x| x == 0));
        Ok(ret)
    }

    /// Decode a base58check-encoded string
    fn from_check(data: &str) -> Result<Vec<u8>, B58Error> {
        let mut ret: Vec<u8> = from(data)?;
        if ret.len() < 4 {
            return Err(B58Error::TooShort(ret.len()));
        }
        let ck_start = ret.len() - 4;
        let expected = LittleEndian::read_u32(&sha256d::Hash::hash(&ret[..ck_start])[..4]);
        let actual = LittleEndian::read_u32(&ret[ck_start..(ck_start + 4)]);
        if expected != actual {
            return Err(B58Error::BadChecksum(expected, actual));
        }

        ret.truncate(ck_start);
        Ok(ret)
    }

    fn format_iter<I, W>(writer: &mut W, data: I) -> Result<(), fmt::Error>
    where
        I: Iterator<Item = u8> + Clone,
        W: fmt::Write,
    {
        let mut ret = SmallVec::new();

        let mut leading_zero_count = 0;
        let mut leading_zeroes = true;
        // Build string in little endian with 0-58 in place of characters...
        for d256 in data {
            let mut carry = d256 as usize;
            if leading_zeroes && carry == 0 {
                leading_zero_count += 1;
            } else {
                leading_zeroes = false;
            }

            for ch in ret.iter_mut() {
                let new_ch = *ch as usize * 256 + carry;
                *ch = (new_ch % 58) as u8;
                carry = new_ch / 58;
            }
            while carry > 0 {
                ret.push((carry % 58) as u8);
                carry /= 58;
            }
        }

        // ... then reverse it and convert to chars
        for _ in 0..leading_zero_count {
            ret.push(0);
        }

        for ch in ret.iter().rev() {
            writer.write_char(BASE58_CHARS[*ch as usize] as char)?;
        }

        Ok(())
    }

    fn encode_iter<I>(data: I) -> String
    where
        I: Iterator<Item = u8> + Clone,
    {
        let mut ret = String::new();
        format_iter(&mut ret, data).expect("writing into string shouldn't fail");
        ret
    }

    /// Directly encode a slice as base58
    #[allow(dead_code)]
    fn encode_slice(data: &[u8]) -> String {
        encode_iter(data.iter().cloned())
    }

    /// Obtain a string with the base58check encoding of a slice
    /// (Tack the first 4 256-digits of the object's Bitcoin hash onto the end.)
    fn check_encode_slice(data: &[u8]) -> String {
        let checksum = sha256d::Hash::hash(&data);
        encode_iter(data.iter().cloned().chain(checksum[0..4].iter().cloned()))
    }

    /// Obtain a string with the base58check encoding of a slice
    /// (Tack the first 4 256-digits of the object's Bitcoin hash onto the end.)
    #[allow(dead_code)]
    fn check_encode_slice_to_fmt(fmt: &mut fmt::Formatter, data: &[u8]) -> fmt::Result {
        let checksum = sha256d::Hash::hash(&data);
        let iter = data.iter().cloned().chain(checksum[0..4].iter().cloned());
        format_iter(fmt, iter)
    }

    impl fmt::Display for ExtendedPrivKey {
        fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
            let mut ret = [0; 78];
            ret[0..4].copy_from_slice(
                &match self.network {
                    // NOTE: WARNING!!! this number is just for test from Bitcoin
                    NetworkType::MainNet => [0x04, 0x88, 0xAD, 0xE4],
                    // NOTE: WARNING!!! this number is just for test from Bitcoin
                    _ => [0x04, 0x35, 0x83, 0x94],
                }[..],
            );
            ret[4] = self.depth as u8;
            ret[5..9].copy_from_slice(&self.parent_fingerprint[..]);

            BigEndian::write_u32(&mut ret[9..13], u32::from(self.child_number));

            ret[13..45].copy_from_slice(&self.chain_code[..]);
            ret[45] = 0;
            ret[46..78].copy_from_slice(&self.private_key[..]);
            fmt.write_str(&check_encode_slice(&ret[..]))
        }
    }

    impl FromStr for ExtendedPrivKey {
        type Err = B58Error;

        fn from_str(inp: &str) -> Result<ExtendedPrivKey, B58Error> {
            let data = from_check(inp)?;

            if data.len() != 78 {
                return Err(B58Error::InvalidLength(data.len()));
            }

            let cn_int: u32 = Cursor::new(&data[9..13]).read_u32::<BigEndian>().unwrap();
            let child_number: ChildNumber = ChildNumber::from(cn_int);

            // NOTE: WARNING!!! this number is just for test from Bitcoin
            let network = if data[0..4] == [0x04u8, 0x88, 0xAD, 0xE4] {
                NetworkType::MainNet
            } else if data[0..4] == [0x04u8, 0x35, 0x83, 0x94] {
                NetworkType::TestNet
            } else {
                return Err(B58Error::InvalidVersion((&data[0..4]).to_vec()));
            };

            Ok(ExtendedPrivKey {
                network,
                depth: data[4],
                parent_fingerprint: Fingerprint::from(&data[5..9]),
                child_number,
                chain_code: ChainCode::from(&data[13..45]),
                private_key: secp256k1::SecretKey::from_slice(&data[46..78])
                    .map_err(|e| B58Error::Other(e.to_string()))?,
            })
        }
    }

    impl fmt::Display for ExtendedPubKey {
        fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
            let mut ret = [0; 78];
            // NOTE: WARNING!!! this number is just for test from Bitcoin
            ret[0..4].copy_from_slice(
                &match self.network {
                    NetworkType::MainNet => [0x04u8, 0x88, 0xB2, 0x1E],
                    _ => [0x04u8, 0x35, 0x87, 0xCF],
                }[..],
            );
            ret[4] = self.depth as u8;
            ret[5..9].copy_from_slice(&self.parent_fingerprint[..]);

            BigEndian::write_u32(&mut ret[9..13], u32::from(self.child_number));

            ret[13..45].copy_from_slice(&self.chain_code[..]);
            ret[45..78].copy_from_slice(&self.public_key.serialize()[..]);
            fmt.write_str(&check_encode_slice(&ret[..]))
        }
    }

    impl FromStr for ExtendedPubKey {
        type Err = B58Error;

        fn from_str(inp: &str) -> Result<ExtendedPubKey, B58Error> {
            let data = from_check(inp)?;

            if data.len() != 78 {
                return Err(B58Error::InvalidLength(data.len()));
            }

            let cn_int: u32 = Cursor::new(&data[9..13]).read_u32::<BigEndian>().unwrap();
            let child_number: ChildNumber = ChildNumber::from(cn_int);

            Ok(ExtendedPubKey {
                // NOTE: WARNING!!! this number is just for test from Bitcoin
                network: if data[0..4] == [0x04u8, 0x88, 0xB2, 0x1E] {
                    NetworkType::MainNet
                } else if data[0..4] == [0x04u8, 0x35, 0x87, 0xCF] {
                    NetworkType::TestNet
                } else {
                    return Err(B58Error::InvalidVersion((&data[0..4]).to_vec()));
                },
                depth: data[4],
                parent_fingerprint: Fingerprint::from(&data[5..9]),
                child_number,
                chain_code: ChainCode::from(&data[13..45]),
                public_key: PublicKey::from_slice(&data[45..78])
                    .map_err(|e| B58Error::Other(e.to_string()))?,
            })
        }
    }

    fn test_path<C: secp256k1::Signing + secp256k1::Verification>(
        secp: &Secp256k1<C>,
        network: NetworkType,
        seed: &[u8],
        path: DerivationPath,
        expected_sk: &str,
        expected_pk: &str,
    ) {
        let mut sk = ExtendedPrivKey::new_master(network, seed).unwrap();
        let mut pk = ExtendedPubKey::from_private(secp, &sk);

        // Check derivation convenience method for ExtendedPrivKey
        assert_eq!(
            &sk.derive_priv(secp, &path).unwrap().to_string()[..],
            expected_sk
        );

        // Check derivation convenience method for ExtendedPubKey, should error
        // appropriately if any ChildNumber is hardened
        if path.0.iter().any(|cnum| cnum.is_hardened()) {
            assert_eq!(
                pk.derive_pub(secp, &path),
                Err(Error::CannotDeriveFromHardenedKey)
            );
        } else {
            assert_eq!(
                &pk.derive_pub(secp, &path).unwrap().to_string()[..],
                expected_pk
            );
        }

        // Derive keys, checking hardened and non-hardened derivation one-by-one
        for &num in path.0.iter() {
            sk = sk.ckd_priv(secp, num).unwrap();
            match num {
                ChildNumber::Normal { .. } => {
                    let pk2 = pk.ckd_pub(secp, num).unwrap();
                    pk = ExtendedPubKey::from_private(secp, &sk);
                    assert_eq!(pk, pk2);
                }
                ChildNumber::Hardened { .. } => {
                    assert_eq!(
                        pk.ckd_pub(secp, num),
                        Err(Error::CannotDeriveFromHardenedKey)
                    );
                    pk = ExtendedPubKey::from_private(secp, &sk);
                }
            }
        }

        // Check result against expected base58
        assert_eq!(&sk.to_string()[..], expected_sk);
        assert_eq!(&pk.to_string()[..], expected_pk);
        // Check decoded base58 against result
        let decoded_sk = ExtendedPrivKey::from_str(expected_sk);
        let decoded_pk = ExtendedPubKey::from_str(expected_pk);
        assert_eq!(Ok(sk), decoded_sk);
        assert_eq!(Ok(pk), decoded_pk);
    }

    #[test]
    fn test_vector_1() {
        let secp = Secp256k1::new();
        let seed_hex = b"000102030405060708090a0b0c0d0e0f";
        let mut seed = vec![0u8; seed_hex.len() / 2];
        hex_decode(seed_hex, &mut seed).unwrap();

        // m
        test_path(&secp, NetworkType::MainNet, &seed, "m".parse().unwrap(),
                  "xprv9s21ZrQH143K3QTDL4LXw2F7HEK3wJUD2nW2nRk4stbPy6cq3jPPqjiChkVvvNKmPGJxWUtg6LnF5kejMRNNU3TGtRBeJgk33yuGBxrMPHi",
                  "xpub661MyMwAqRbcFtXgS5sYJABqqG9YLmC4Q1Rdap9gSE8NqtwybGhePY2gZ29ESFjqJoCu1Rupje8YtGqsefD265TMg7usUDFdp6W1EGMcet8");

        // m/0h
        test_path(&secp, NetworkType::MainNet, &seed, "m/0h".parse().unwrap(),
                  "xprv9uHRZZhk6KAJC1avXpDAp4MDc3sQKNxDiPvvkX8Br5ngLNv1TxvUxt4cV1rGL5hj6KCesnDYUhd7oWgT11eZG7XnxHrnYeSvkzY7d2bhkJ7",
                  "xpub68Gmy5EdvgibQVfPdqkBBCHxA5htiqg55crXYuXoQRKfDBFA1WEjWgP6LHhwBZeNK1VTsfTFUHCdrfp1bgwQ9xv5ski8PX9rL2dZXvgGDnw");

        // m/0h/1
        test_path(&secp, NetworkType::MainNet, &seed, "m/0h/1".parse().unwrap(),
                   "xprv9wTYmMFdV23N2TdNG573QoEsfRrWKQgWeibmLntzniatZvR9BmLnvSxqu53Kw1UmYPxLgboyZQaXwTCg8MSY3H2EU4pWcQDnRnrVA1xe8fs",
                   "xpub6ASuArnXKPbfEwhqN6e3mwBcDTgzisQN1wXN9BJcM47sSikHjJf3UFHKkNAWbWMiGj7Wf5uMash7SyYq527Hqck2AxYysAA7xmALppuCkwQ");

        // m/0h/1/2h
        test_path(&secp, NetworkType::MainNet, &seed, "m/0h/1/2h".parse().unwrap(),
                  "xprv9z4pot5VBttmtdRTWfWQmoH1taj2axGVzFqSb8C9xaxKymcFzXBDptWmT7FwuEzG3ryjH4ktypQSAewRiNMjANTtpgP4mLTj34bhnZX7UiM",
                  "xpub6D4BDPcP2GT577Vvch3R8wDkScZWzQzMMUm3PWbmWvVJrZwQY4VUNgqFJPMM3No2dFDFGTsxxpG5uJh7n7epu4trkrX7x7DogT5Uv6fcLW5");

        // m/0h/1/2h/2
        test_path(&secp, NetworkType::MainNet, &seed, "m/0h/1/2h/2".parse().unwrap(),
                  "xprvA2JDeKCSNNZky6uBCviVfJSKyQ1mDYahRjijr5idH2WwLsEd4Hsb2Tyh8RfQMuPh7f7RtyzTtdrbdqqsunu5Mm3wDvUAKRHSC34sJ7in334",
                  "xpub6FHa3pjLCk84BayeJxFW2SP4XRrFd1JYnxeLeU8EqN3vDfZmbqBqaGJAyiLjTAwm6ZLRQUMv1ZACTj37sR62cfN7fe5JnJ7dh8zL4fiyLHV");

        // m/0h/1/2h/2/1000000000
        test_path(&secp, NetworkType::MainNet, &seed, "m/0h/1/2h/2/1000000000".parse().unwrap(),
                  "xprvA41z7zogVVwxVSgdKUHDy1SKmdb533PjDz7J6N6mV6uS3ze1ai8FHa8kmHScGpWmj4WggLyQjgPie1rFSruoUihUZREPSL39UNdE3BBDu76",
                  "xpub6H1LXWLaKsWFhvm6RVpEL9P4KfRZSW7abD2ttkWP3SSQvnyA8FSVqNTEcYFgJS2UaFcxupHiYkro49S8yGasTvXEYBVPamhGW6cFJodrTHy");
    }

    #[test]
    fn test_vector_2() {
        let secp = Secp256k1::new();
        let seed_hex = b"fffcf9f6f3f0edeae7e4e1dedbd8d5d2cfccc9c6c3c0bdbab7b4b1aeaba8a5a29f9c999693908d8a8784817e7b7875726f6c696663605d5a5754514e4b484542";
        let mut seed = vec![0u8; seed_hex.len() / 2];
        hex_decode(seed_hex, &mut seed).unwrap();

        // m
        test_path(&secp, NetworkType::MainNet, &seed, "m".parse().unwrap(),
                  "xprv9s21ZrQH143K31xYSDQpPDxsXRTUcvj2iNHm5NUtrGiGG5e2DtALGdso3pGz6ssrdK4PFmM8NSpSBHNqPqm55Qn3LqFtT2emdEXVYsCzC2U",
                  "xpub661MyMwAqRbcFW31YEwpkMuc5THy2PSt5bDMsktWQcFF8syAmRUapSCGu8ED9W6oDMSgv6Zz8idoc4a6mr8BDzTJY47LJhkJ8UB7WEGuduB");

        // m/0
        test_path(&secp, NetworkType::MainNet, &seed, "m/0".parse().unwrap(),
                  "xprv9vHkqa6EV4sPZHYqZznhT2NPtPCjKuDKGY38FBWLvgaDx45zo9WQRUT3dKYnjwih2yJD9mkrocEZXo1ex8G81dwSM1fwqWpWkeS3v86pgKt",
                  "xpub69H7F5d8KSRgmmdJg2KhpAK8SR3DjMwAdkxj3ZuxV27CprR9LgpeyGmXUbC6wb7ERfvrnKZjXoUmmDznezpbZb7ap6r1D3tgFxHmwMkQTPH");

        // m/0/2147483647h
        test_path(&secp, NetworkType::MainNet, &seed, "m/0/2147483647h".parse().unwrap(),
                  "xprv9wSp6B7kry3Vj9m1zSnLvN3xH8RdsPP1Mh7fAaR7aRLcQMKTR2vidYEeEg2mUCTAwCd6vnxVrcjfy2kRgVsFawNzmjuHc2YmYRmagcEPdU9",
                  "xpub6ASAVgeehLbnwdqV6UKMHVzgqAG8Gr6riv3Fxxpj8ksbH9ebxaEyBLZ85ySDhKiLDBrQSARLq1uNRts8RuJiHjaDMBU4Zn9h8LZNnBC5y4a");

        // m/0/2147483647h/1
        test_path(&secp, NetworkType::MainNet, &seed, "m/0/2147483647h/1".parse().unwrap(),
                  "xprv9zFnWC6h2cLgpmSA46vutJzBcfJ8yaJGg8cX1e5StJh45BBciYTRXSd25UEPVuesF9yog62tGAQtHjXajPPdbRCHuWS6T8XA2ECKADdw4Ef",
                  "xpub6DF8uhdarytz3FWdA8TvFSvvAh8dP3283MY7p2V4SeE2wyWmG5mg5EwVvmdMVCQcoNJxGoWaU9DCWh89LojfZ537wTfunKau47EL2dhHKon");

        // m/0/2147483647h/1/2147483646h
        test_path(&secp, NetworkType::MainNet, &seed, "m/0/2147483647h/1/2147483646h".parse().unwrap(),
                  "xprvA1RpRA33e1JQ7ifknakTFpgNXPmW2YvmhqLQYMmrj4xJXXWYpDPS3xz7iAxn8L39njGVyuoseXzU6rcxFLJ8HFsTjSyQbLYnMpCqE2VbFWc",
                  "xpub6ERApfZwUNrhLCkDtcHTcxd75RbzS1ed54G1LkBUHQVHQKqhMkhgbmJbZRkrgZw4koxb5JaHWkY4ALHY2grBGRjaDMzQLcgJvLJuZZvRcEL");

        // m/0/2147483647h/1/2147483646h/2
        test_path(&secp, NetworkType::MainNet, &seed, "m/0/2147483647h/1/2147483646h/2".parse().unwrap(),
                  "xprvA2nrNbFZABcdryreWet9Ea4LvTJcGsqrMzxHx98MMrotbir7yrKCEXw7nadnHM8Dq38EGfSh6dqA9QWTyefMLEcBYJUuekgW4BYPJcr9E7j",
                  "xpub6FnCn6nSzZAw5Tw7cgR9bi15UV96gLZhjDstkXXxvCLsUXBGXPdSnLFbdpq8p9HmGsApME5hQTZ3emM2rnY5agb9rXpVGyy3bdW6EEgAtqt");
    }

    #[test]
    fn test_vector_3() {
        let secp = Secp256k1::new();
        let seed_hex = b"4b381541583be4423346c643850da4b320e46a87ae3d2a4e6da11eba819cd4acba45d239319ac14f863b8d5ab5a0d0c64d2e8a1e7d1457df2e5a3c51c73235be";
        let mut seed = vec![0u8; seed_hex.len() / 2];
        hex_decode(seed_hex, &mut seed).unwrap();

        // m
        test_path(&secp, NetworkType::MainNet, &seed, "m".parse().unwrap(),
                  "xprv9s21ZrQH143K25QhxbucbDDuQ4naNntJRi4KUfWT7xo4EKsHt2QJDu7KXp1A3u7Bi1j8ph3EGsZ9Xvz9dGuVrtHHs7pXeTzjuxBrCmmhgC6",
                  "xpub661MyMwAqRbcEZVB4dScxMAdx6d4nFc9nvyvH3v4gJL378CSRZiYmhRoP7mBy6gSPSCYk6SzXPTf3ND1cZAceL7SfJ1Z3GC8vBgp2epUt13");

        // m/0h
        test_path(&secp, NetworkType::MainNet, &seed, "m/0h".parse().unwrap(),
                  "xprv9uPDJpEQgRQfDcW7BkF7eTya6RPxXeJCqCJGHuCJ4GiRVLzkTXBAJMu2qaMWPrS7AANYqdq6vcBcBUdJCVVFceUvJFjaPdGZ2y9WACViL4L",
                  "xpub68NZiKmJWnxxS6aaHmn81bvJeTESw724CRDs6HbuccFQN9Ku14VQrADWgqbhhTHBaohPX4CjNLf9fq9MYo6oDaPPLPxSb7gwQN3ih19Zm4Y");
    }

    #[test]
    fn test_parse_derivation_path() {
        assert_eq!(
            DerivationPath::from_str("42"),
            Err(Error::InvalidDerivationPathFormat)
        );
        assert_eq!(
            DerivationPath::from_str("n/0'/0"),
            Err(Error::InvalidDerivationPathFormat)
        );
        assert_eq!(
            DerivationPath::from_str("4/m/5"),
            Err(Error::InvalidDerivationPathFormat)
        );
        assert_eq!(
            DerivationPath::from_str("m//3/0'"),
            Err(Error::InvalidChildNumberFormat)
        );
        assert_eq!(
            DerivationPath::from_str("m/0h/0x"),
            Err(Error::InvalidChildNumberFormat)
        );
        assert_eq!(
            DerivationPath::from_str("m/2147483648"),
            Err(Error::InvalidChildNumber(2_147_483_648))
        );

        assert_eq!(DerivationPath::from_str("m"), Ok(vec![].into()));
        assert_eq!(
            DerivationPath::from_str("m/0'"),
            Ok(vec![ChildNumber::from_hardened_idx(0).unwrap()].into())
        );
        assert_eq!(
            DerivationPath::from_str("m/0'/1"),
            Ok(vec![
                ChildNumber::from_hardened_idx(0).unwrap(),
                ChildNumber::from_normal_idx(1).unwrap()
            ]
            .into())
        );
        assert_eq!(
            DerivationPath::from_str("m/0h/1/2'"),
            Ok(vec![
                ChildNumber::from_hardened_idx(0).unwrap(),
                ChildNumber::from_normal_idx(1).unwrap(),
                ChildNumber::from_hardened_idx(2).unwrap(),
            ]
            .into())
        );
        assert_eq!(
            DerivationPath::from_str("m/0'/1/2h/2"),
            Ok(vec![
                ChildNumber::from_hardened_idx(0).unwrap(),
                ChildNumber::from_normal_idx(1).unwrap(),
                ChildNumber::from_hardened_idx(2).unwrap(),
                ChildNumber::from_normal_idx(2).unwrap(),
            ]
            .into())
        );
        assert_eq!(
            DerivationPath::from_str("m/0'/1/2'/2/1000000000"),
            Ok(vec![
                ChildNumber::from_hardened_idx(0).unwrap(),
                ChildNumber::from_normal_idx(1).unwrap(),
                ChildNumber::from_hardened_idx(2).unwrap(),
                ChildNumber::from_normal_idx(2).unwrap(),
                ChildNumber::from_normal_idx(1_000_000_000).unwrap(),
            ]
            .into())
        );
    }

    #[test]
    fn test_derivation_path_convertion_index() {
        let path = DerivationPath::from_str("m/0h/1/2'").unwrap();
        let numbers: Vec<ChildNumber> = path.clone().into();
        let path2: DerivationPath = numbers.into();
        assert_eq!(path, path2);
        assert_eq!(
            &path[..2],
            &[
                ChildNumber::from_hardened_idx(0).unwrap(),
                ChildNumber::from_normal_idx(1).unwrap()
            ]
        );
        let indexed: DerivationPath = path[..2].into();
        assert_eq!(indexed, DerivationPath::from_str("m/0h/1").unwrap());
        assert_eq!(
            indexed.child(ChildNumber::from_hardened_idx(2).unwrap()),
            path
        );
    }

    #[test]
    fn test_increment() {
        let idx = 9_345_497; // randomly generated, I promise
        let cn = ChildNumber::from_normal_idx(idx).unwrap();
        assert_eq!(
            cn.increment().ok(),
            Some(ChildNumber::from_normal_idx(idx + 1).unwrap())
        );
        let cn = ChildNumber::from_hardened_idx(idx).unwrap();
        assert_eq!(
            cn.increment().ok(),
            Some(ChildNumber::from_hardened_idx(idx + 1).unwrap())
        );

        let max = (1 << 31) - 1;
        let cn = ChildNumber::from_normal_idx(max).unwrap();
        assert_eq!(
            cn.increment().err(),
            Some(Error::InvalidChildNumber(1 << 31))
        );
        let cn = ChildNumber::from_hardened_idx(max).unwrap();
        assert_eq!(
            cn.increment().err(),
            Some(Error::InvalidChildNumber(1 << 31))
        );

        let cn = ChildNumber::from_normal_idx(350).unwrap();
        let path = DerivationPath::from_str("m/42'").unwrap();
        let mut iter = path.children_from(cn);
        assert_eq!(iter.next(), Some("m/42'/350".parse().unwrap()));
        assert_eq!(iter.next(), Some("m/42'/351".parse().unwrap()));

        let path = DerivationPath::from_str("m/42'/350'").unwrap();
        let mut iter = path.normal_children();
        assert_eq!(iter.next(), Some("m/42'/350'/0".parse().unwrap()));
        assert_eq!(iter.next(), Some("m/42'/350'/1".parse().unwrap()));

        let path = DerivationPath::from_str("m/42'/350'").unwrap();
        let mut iter = path.hardened_children();
        assert_eq!(iter.next(), Some("m/42'/350'/0'".parse().unwrap()));
        assert_eq!(iter.next(), Some("m/42'/350'/1'".parse().unwrap()));

        let cn = ChildNumber::from_hardened_idx(42350).unwrap();
        let path = DerivationPath::from_str("m/42'").unwrap();
        let mut iter = path.children_from(cn);
        assert_eq!(iter.next(), Some("m/42'/42350'".parse().unwrap()));
        assert_eq!(iter.next(), Some("m/42'/42351'".parse().unwrap()));

        let cn = ChildNumber::from_hardened_idx(max).unwrap();
        let path = DerivationPath::from_str("m/42'").unwrap();
        let mut iter = path.children_from(cn);
        assert!(iter.next().is_some());
        assert!(iter.next().is_none());
    }
}
