//! Ed25519 (RFC 8032): the signature on each update.
//!
//! The publisher signs the bytes of an artifact with a secret key. The
//! app holds the public key from the day it was built and accepts an
//! artifact only when the signature verifies.
//!
//! ## Two backends, one behavior
//!
//! The default backend is in this crate, on the standard library alone.
//! The `dalek` feature puts `ed25519-dalek` behind the same API. Both
//! accept and refuse the same signatures, the rules of
//! `ed25519_dalek::VerifyingKey::verify`:
//!
//! - The equation has no cofactor: `[s]B = R + [k]A`, compared as the
//!   32 bytes of R.
//! - `s` must be below the group order l.
//! - A public key is any point that decompresses, small order too.
//!
//! The tests hold the two backends to the same answer.
//!
//! ## Production gotchas
//!
//! **The secret stays in memory until the process ends.** Signing and
//! key generation do the same work for every secret, so the time does
//! not show it. But the crate has no unsafe code, so it cannot clear
//! memory past the optimizer. Sign on a machine that you trust.

mod edwards;
mod field;
mod scalar;

#[cfg(test)]
mod tests;

use std::io::{self, Read};

use crate::sha512::Sha512;

pub const SEED_LENGTH: usize = 32;
pub const PUBLIC_KEY_LENGTH: usize = 32;
pub const SIGNATURE_LENGTH: usize = 64;

/// A secret key: the 32-byte seed and what RFC 8032 derives from it.
#[derive(Clone)]
pub struct SigningKey {
    seed: [u8; 32],
    /// The secret scalar a, clamped.
    scalar: [u8; 32],
    /// The second half of SHA-512(seed), which makes the nonce.
    prefix: [u8; 32],
    public: VerifyingKey,
}

impl std::fmt::Debug for SigningKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SigningKey").field("public", &self.public).finish_non_exhaustive()
    }
}

impl SigningKey {
    /// The key of a 32-byte seed.
    #[must_use]
    pub fn from_seed(seed: &[u8; SEED_LENGTH]) -> SigningKey {
        let hash = Sha512::new().chain(seed).finish();
        let mut scalar = [0u8; 32];
        scalar.copy_from_slice(&hash[..32]);
        scalar[0] &= 248;
        scalar[31] &= 127;
        scalar[31] |= 64;
        let mut prefix = [0u8; 32];
        prefix.copy_from_slice(&hash[32..]);
        let public = VerifyingKey { bytes: backend::public_key(seed, &scalar) };
        SigningKey { seed: *seed, scalar, prefix, public }
    }

    /// A new key, from the system's random bytes.
    ///
    /// # Errors
    ///
    /// An error when the system generator cannot be read.
    pub fn generate() -> io::Result<SigningKey> {
        let mut seed = [0u8; SEED_LENGTH];
        crate::random::fill(&mut seed)?;
        Ok(SigningKey::from_seed(&seed))
    }

    /// The seed: the only part to keep, the rest comes from it.
    #[must_use]
    pub fn seed(&self) -> &[u8; SEED_LENGTH] {
        &self.seed
    }

    #[must_use]
    pub fn verifying_key(&self) -> VerifyingKey {
        self.public
    }

    /// The signature of `message`.
    #[must_use]
    pub fn sign(&self, message: &[u8]) -> [u8; SIGNATURE_LENGTH] {
        backend::sign(self, message)
    }
}

/// A public key: a point of the curve, kept as its 32 bytes.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct VerifyingKey {
    bytes: [u8; PUBLIC_KEY_LENGTH],
}

impl std::fmt::Debug for VerifyingKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "VerifyingKey({})", crate::sha::hex(&self.bytes))
    }
}

impl VerifyingKey {
    /// The key of 32 bytes, or `None` when they are not a point of the
    /// curve.
    #[must_use]
    pub fn from_bytes(bytes: &[u8; PUBLIC_KEY_LENGTH]) -> Option<VerifyingKey> {
        edwards::Point::decompress(bytes).map(|_| VerifyingKey { bytes: *bytes })
    }

    #[must_use]
    pub fn as_bytes(&self) -> &[u8; PUBLIC_KEY_LENGTH] {
        &self.bytes
    }

    /// Whether `signature` is this key's signature of `message`.
    #[must_use]
    pub fn verify(&self, message: &[u8], signature: &[u8; SIGNATURE_LENGTH]) -> bool {
        backend::verify(self, message, signature)
    }

    /// Whether `signature` is this key's signature of all the bytes of
    /// `message`, read to the end.
    ///
    /// The default backend hashes the bytes as they come, so a file of
    /// any size takes little memory. The `dalek` backend reads it whole
    /// first.
    ///
    /// # Errors
    ///
    /// An error when `message` cannot be read.
    pub fn verify_reader(&self, message: impl Read, signature: &[u8; SIGNATURE_LENGTH]) -> io::Result<bool> {
        backend::verify_reader(self, message, signature)
    }
}

/// The backend in this crate. It is always built: the tests hold the
/// `dalek` backend to its answers.
#[cfg_attr(feature = "dalek", allow(dead_code))]
mod own {
    use super::edwards::Point;
    use super::{SIGNATURE_LENGTH, SigningKey, VerifyingKey, scalar};
    use crate::sha512::Sha512;
    use std::io::{self, Read};

    pub fn public_key(_seed: &[u8; 32], secret: &[u8; 32]) -> [u8; 32] {
        Point::base().mul(secret).compress()
    }

    pub fn sign(key: &SigningKey, message: &[u8]) -> [u8; SIGNATURE_LENGTH] {
        let r = scalar::from_hash(&Sha512::new().chain(&key.prefix).chain(message).finish());
        let big_r = Point::base().mul(&r).compress();
        let k = challenge(Sha512::new().chain(&big_r).chain(key.public.as_bytes()).chain(message));
        let s = scalar::mul_add(&k, &key.scalar, &r);
        let mut out = [0u8; SIGNATURE_LENGTH];
        out[..32].copy_from_slice(&big_r);
        out[32..].copy_from_slice(&s);
        out
    }

    pub fn verify(key: &VerifyingKey, message: &[u8], signature: &[u8; SIGNATURE_LENGTH]) -> bool {
        let Some(hash) = start(key, signature) else {
            return false;
        };
        finish(key, hash.chain(message), signature)
    }

    pub fn verify_reader(
        key: &VerifyingKey,
        mut message: impl Read,
        signature: &[u8; SIGNATURE_LENGTH],
    ) -> io::Result<bool> {
        let Some(mut hash) = start(key, signature) else {
            return Ok(false);
        };
        let mut chunk = vec![0u8; 1 << 16];
        loop {
            let read = match message.read(&mut chunk) {
                Ok(0) => break,
                Ok(read) => read,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            };
            hash.update(&chunk[..read]);
        }
        Ok(finish(key, hash, signature))
    }

    /// The hash of R and A, ready for the message; `None` when s is not
    /// below l.
    fn start(key: &VerifyingKey, signature: &[u8; SIGNATURE_LENGTH]) -> Option<Sha512> {
        let s: &[u8; 32] = signature[32..].try_into().ok()?;
        scalar::is_canonical(s).then(|| Sha512::new().chain(&signature[..32]).chain(key.as_bytes()))
    }

    fn finish(key: &VerifyingKey, hash: Sha512, signature: &[u8; SIGNATURE_LENGTH]) -> bool {
        let Some(a) = Point::decompress(key.as_bytes()) else {
            return false;
        };
        let mut s = [0u8; 32];
        s.copy_from_slice(&signature[32..]);
        let k = challenge(hash);
        // [s]B - [k]A is R when the signature is good
        let r = Point::base().mul(&s).add(a.neg().mul(&k));
        r.compress()[..] == signature[..32]
    }

    fn challenge(hash: Sha512) -> [u8; 32] {
        scalar::from_hash(&hash.finish())
    }
}

#[cfg(feature = "dalek")]
mod dalek {
    use super::{SIGNATURE_LENGTH, SigningKey, VerifyingKey};
    use std::io::{self, Read};

    pub fn public_key(seed: &[u8; 32], _secret: &[u8; 32]) -> [u8; 32] {
        ed25519_dalek::SigningKey::from_bytes(seed).verifying_key().to_bytes()
    }

    pub fn sign(key: &SigningKey, message: &[u8]) -> [u8; SIGNATURE_LENGTH] {
        use ed25519_dalek::Signer as _;
        ed25519_dalek::SigningKey::from_bytes(&key.seed).sign(message).to_bytes()
    }

    pub fn verify(key: &VerifyingKey, message: &[u8], signature: &[u8; SIGNATURE_LENGTH]) -> bool {
        use ed25519_dalek::Verifier as _;
        let Ok(key) = ed25519_dalek::VerifyingKey::from_bytes(key.as_bytes()) else {
            return false;
        };
        key.verify(message, &ed25519_dalek::Signature::from_bytes(signature)).is_ok()
    }

    pub fn verify_reader(
        key: &VerifyingKey,
        mut message: impl Read,
        signature: &[u8; SIGNATURE_LENGTH],
    ) -> io::Result<bool> {
        let mut bytes = Vec::new();
        message.read_to_end(&mut bytes)?;
        Ok(verify(key, &bytes, signature))
    }
}

#[cfg(not(feature = "dalek"))]
use own as backend;

#[cfg(feature = "dalek")]
use dalek as backend;
