//! ECDH P-256 key agreement + HKDF-SHA256 key derivation, byte-for-byte
//! compatible with secure-send-web's `src/lib/crypto/ecdh.ts`.
//!
//! The AES content key is derived as:
//! `HKDF-SHA256(ikm = ECDH shared X coordinate, salt = 16-byte transfer salt,
//!  info = "secure-send-mutual", len = 32)`.

use anyhow::{Result, bail};
use hkdf::Hkdf;
use p256::PublicKey;
use p256::elliptic_curve::sec1::ToEncodedPoint;
use sha2::Sha256;

use super::chunk::fill_random;

/// HKDF `info` string for the mutual (manual-mode) content key.
const HKDF_INFO_MUTUAL: &[u8] = b"secure-send-mutual";
/// Transfer salt length (`SALT_LENGTH`).
pub const SALT_LEN: usize = 16;
/// Uncompressed P-256 public key length (`0x04 || X || Y`).
pub const PUBLIC_KEY_LEN: usize = 65;

/// An ephemeral ECDH key pair. The secret scalar never leaves this struct.
pub struct EcdhKeyPair {
    secret: p256::SecretKey,
    /// 65-byte uncompressed public key (`0x04 || X(32) || Y(32)`).
    pub public_key_bytes: [u8; PUBLIC_KEY_LEN],
}

impl EcdhKeyPair {
    /// Generate a fresh ephemeral P-256 key pair.
    pub fn generate() -> Result<Self> {
        // Rejection-sample a valid non-zero scalar < curve order. A random
        // 32-byte string is out of range with probability ~2^-32, so this
        // effectively never loops.
        let secret = loop {
            let mut bytes = [0u8; 32];
            fill_random(&mut bytes)?;
            if let Ok(sk) = p256::SecretKey::from_slice(&bytes) {
                break sk;
            }
        };

        let encoded = secret.public_key().to_encoded_point(false);
        let mut public_key_bytes = [0u8; PUBLIC_KEY_LEN];
        public_key_bytes.copy_from_slice(encoded.as_bytes());

        Ok(Self {
            secret,
            public_key_bytes,
        })
    }

    /// Derive the shared AES-256 key from the peer's public key and the
    /// per-transfer salt.
    pub fn derive_aes_key(&self, peer_public_key: &[u8], salt: &[u8]) -> Result<[u8; 32]> {
        if salt.len() < SALT_LEN {
            bail!("salt too short: expected at least {SALT_LEN} bytes, got {}", salt.len());
        }
        let peer = import_public_key(peer_public_key)?;

        let shared = p256::ecdh::diffie_hellman(self.secret.to_nonzero_scalar(), peer.as_affine());
        // secure-send-web's Web Crypto ECDH yields the X coordinate as the HKDF
        // input keying material; `raw_secret_bytes()` is exactly that X coordinate.
        let ikm = shared.raw_secret_bytes();

        let hk = Hkdf::<Sha256>::new(Some(salt), ikm);
        let mut okm = [0u8; 32];
        hk.expand(HKDF_INFO_MUTUAL, &mut okm)
            .map_err(|_| anyhow::anyhow!("HKDF expand failed"))?;
        Ok(okm)
    }
}

/// Import a peer's 65-byte uncompressed P-256 public key, validating format and
/// that the point is on the curve.
pub fn import_public_key(bytes: &[u8]) -> Result<PublicKey> {
    if bytes.len() != PUBLIC_KEY_LEN {
        bail!(
            "invalid ECDH public key: expected {PUBLIC_KEY_LEN}-byte uncompressed key, got {}",
            bytes.len()
        );
    }
    if bytes[0] != 0x04 {
        bail!("invalid ECDH public key: missing uncompressed point prefix (0x04)");
    }
    PublicKey::from_sec1_bytes(bytes)
        .map_err(|_| anyhow::anyhow!("invalid ECDH public key: point not on curve"))
}

/// Generate a fresh 16-byte transfer salt.
pub fn generate_salt() -> Result<[u8; SALT_LEN]> {
    let mut salt = [0u8; SALT_LEN];
    fill_random(&mut salt)?;
    Ok(salt)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_sides_derive_same_key() {
        let alice = EcdhKeyPair::generate().unwrap();
        let bob = EcdhKeyPair::generate().unwrap();
        let salt = generate_salt().unwrap();

        let ka = alice
            .derive_aes_key(&bob.public_key_bytes, &salt)
            .unwrap();
        let kb = bob
            .derive_aes_key(&alice.public_key_bytes, &salt)
            .unwrap();
        assert_eq!(ka, kb);
    }

    #[test]
    fn different_salt_differs() {
        let alice = EcdhKeyPair::generate().unwrap();
        let bob = EcdhKeyPair::generate().unwrap();
        let k1 = alice.derive_aes_key(&bob.public_key_bytes, &[9u8; 16]).unwrap();
        let k2 = alice.derive_aes_key(&bob.public_key_bytes, &[8u8; 16]).unwrap();
        assert_ne!(k1, k2);
    }

    #[test]
    fn rejects_bad_public_key() {
        assert!(import_public_key(&[0u8; 10]).is_err());
        assert!(import_public_key(&[0u8; 65]).is_err()); // prefix != 0x04
    }
}
