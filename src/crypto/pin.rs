//! PIN generation, PIN hints, and PIN-derived keys for secure-send-web's
//! Nostr "Auto Exchange" mode.

use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Result, bail};
use pbkdf2::pbkdf2_hmac;
use sha2::Sha256;

use super::aes::AES_KEY_LEN;
use super::chunk::fill_random;

pub const PIN_LENGTH: usize = 12;
const PIN_CHECKSUM_LENGTH: usize = 1;
const PIN_CHARSET: &[u8] =
    b"ABCDEFGHJKLMNPQRSTUVWXYZabcdefghjkmnpqrstuvwxyz23456789-/:;()$&@?!.,\"";
const PBKDF2_ITERATIONS: u32 = 600_000;
const SALT_LENGTH: usize = 16;
const PIN_HINT_LENGTH: usize = 16;
const PIN_HINT_BUCKET_SEC: u64 = 3600;
const PIN_HINT_SALT: &str = "secure-send:pin-hint:v1";
const PIN_KEY_LABEL_CONTEXT: &str = "secure-send:pin-key:v1";

pub const TRANSFER_EXPIRATION_MS: u64 = 60 * 60 * 1000;

#[derive(Debug, Clone)]
pub struct NostrTransferKeys {
    pub metadata: [u8; AES_KEY_LEN],
    pub signals: [u8; AES_KEY_LEN],
    pub p2p_content: [u8; AES_KEY_LEN],
}

#[derive(Debug, Clone, Copy)]
enum PinKeyLabel {
    Metadata,
    Signals,
    P2pContent,
}

impl PinKeyLabel {
    fn as_str(self) -> &'static str {
        match self {
            Self::Metadata => "metadata",
            Self::Signals => "signals",
            Self::P2pContent => "p2p-content",
        }
    }
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before Unix epoch")
        .as_millis() as u64
}

pub fn now_sec() -> u64 {
    now_ms() / 1000
}

pub fn is_expired(created_at_ms: u64) -> bool {
    now_ms().saturating_sub(created_at_ms) > TRANSFER_EXPIRATION_MS
}

fn compute_checksum(data: &[u8]) -> u8 {
    let mut sum = 0usize;
    for (i, byte) in data.iter().enumerate() {
        let Some(index) = PIN_CHARSET.iter().position(|c| c == byte) else {
            return PIN_CHARSET[0];
        };
        sum += index * (i + 1);
    }
    PIN_CHARSET[sum % PIN_CHARSET.len()]
}

pub fn generate_pin() -> Result<String> {
    let data_len = PIN_LENGTH - PIN_CHECKSUM_LENGTH;
    let charset_len = PIN_CHARSET.len();
    let max_multiple = (256 / charset_len) * charset_len;
    let mut data = Vec::with_capacity(data_len);
    let mut buf = vec![0u8; data_len * 2];

    while data.len() < data_len {
        fill_random(&mut buf)?;
        for byte in &buf {
            let n = *byte as usize;
            if n < max_multiple {
                data.push(PIN_CHARSET[n % charset_len]);
                if data.len() == data_len {
                    break;
                }
            }
        }
    }

    data.push(compute_checksum(&data));
    String::from_utf8(data).map_err(|e| anyhow::anyhow!("generated invalid PIN: {e}"))
}

pub fn is_valid_pin(pin: &str) -> bool {
    let bytes = pin.as_bytes();
    if bytes.len() != PIN_LENGTH {
        return false;
    }
    if !bytes.iter().all(|byte| PIN_CHARSET.contains(byte)) {
        return false;
    }

    let data = &bytes[..PIN_LENGTH - PIN_CHECKSUM_LENGTH];
    compute_checksum(data) == bytes[PIN_LENGTH - PIN_CHECKSUM_LENGTH]
}

pub fn generate_salt() -> Result<[u8; SALT_LENGTH]> {
    let mut salt = [0u8; SALT_LENGTH];
    fill_random(&mut salt)?;
    Ok(salt)
}

pub fn generate_transfer_id() -> Result<String> {
    let mut bytes = [0u8; 8];
    fill_random(&mut bytes)?;
    Ok(hex_lower(&bytes))
}

pub fn compute_pin_hint(pin: &str, bucket_offset: u64) -> String {
    let bucket = now_sec()
        .checked_div(PIN_HINT_BUCKET_SEC)
        .unwrap_or_default()
        .saturating_sub(bucket_offset);
    let salt = format!("{PIN_HINT_SALT}:{bucket}");
    let mut out = vec![0u8; PIN_HINT_LENGTH.div_ceil(2)];
    pbkdf2_hmac::<Sha256>(pin.as_bytes(), salt.as_bytes(), PBKDF2_ITERATIONS, &mut out);
    hex_lower(&out)[..PIN_HINT_LENGTH].to_string()
}

pub fn derive_nostr_transfer_keys(pin: &str, salt: &[u8]) -> Result<NostrTransferKeys> {
    Ok(NostrTransferKeys {
        metadata: derive_labeled_key(pin, salt, PinKeyLabel::Metadata)?,
        signals: derive_labeled_key(pin, salt, PinKeyLabel::Signals)?,
        p2p_content: derive_labeled_key(pin, salt, PinKeyLabel::P2pContent)?,
    })
}

fn derive_labeled_key(pin: &str, salt: &[u8], label: PinKeyLabel) -> Result<[u8; AES_KEY_LEN]> {
    if salt.len() < SALT_LENGTH {
        bail!(
            "salt too short: expected at least {SALT_LENGTH} bytes, got {}",
            salt.len()
        );
    }

    let label = format!("{PIN_KEY_LABEL_CONTEXT}:{}", label.as_str());
    let mut labeled_salt = Vec::with_capacity(salt.len() + 1 + label.len());
    labeled_salt.extend_from_slice(salt);
    labeled_salt.push(0);
    labeled_salt.extend_from_slice(label.as_bytes());

    let mut out = [0u8; AES_KEY_LEN];
    pbkdf2_hmac::<Sha256>(pin.as_bytes(), &labeled_salt, PBKDF2_ITERATIONS, &mut out);
    Ok(out)
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_pin_validates() {
        let pin = generate_pin().unwrap();
        assert_eq!(pin.len(), PIN_LENGTH);
        assert!(is_valid_pin(&pin));
    }

    #[test]
    fn checksum_rejects_typo() {
        let mut pin = generate_pin().unwrap().into_bytes();
        pin[0] = if pin[0] == b'A' { b'B' } else { b'A' };
        assert!(!is_valid_pin(std::str::from_utf8(&pin).unwrap()));
    }

    #[test]
    fn labels_are_domain_separated() {
        let pin = "ABCDEFGHJKL2";
        let salt = [7u8; SALT_LENGTH];
        let keys = derive_nostr_transfer_keys(pin, &salt).unwrap();
        assert_ne!(keys.metadata, keys.signals);
        assert_ne!(keys.signals, keys.p2p_content);
    }
}
