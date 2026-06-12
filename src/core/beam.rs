use anyhow::{Context, Result};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use std::time::{SystemTime, UNIX_EPOCH};

/// Current token format version
pub const CURRENT_VERSION: u8 = 5;

/// TTL for beam sessions in seconds (1 hour)
pub const SESSION_TTL_SECS: u64 = 3600;

/// Minimum base64url-encoded beam code length.
/// A minimal token payload is ~20+ bytes, which base64 encodes to ~30+ characters.
const MIN_CODE_LENGTH: usize = 30;

/// Beam token containing WebRTC signaling metadata.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BeamToken {
    /// Token format version.
    pub version: u8,
    /// Unix timestamp when this token was created (for TTL validation)
    pub created_at: u64,
    /// Sender's ephemeral Nostr public key for signaling (hex)
    pub sender_pubkey: String,
    /// Unique transfer session ID
    pub transfer_id: String,
    /// List of Nostr relay URLs for signaling
    pub relays: Vec<String>,
    /// Transfer type: "file" or "folder"
    pub transfer_type: String,
    /// Original filename.
    pub filename: String,
}

/// Get current Unix timestamp in seconds
pub fn current_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("System clock is set before Unix epoch")
        .as_secs()
}

/// Generate a beam code for webrtc transfer (WebRTC + Nostr signaling)
/// Format: base64url(json(BeamToken))
///
/// # Arguments
/// * `sender_pubkey` - Sender's ephemeral Nostr public key for signaling (hex)
/// * `transfer_id` - Unique transfer session ID
/// * `relays` - List of Nostr relay URLs for signaling
/// * `filename` - Original filename
/// * `transfer_type` - "file" or "folder"
///
/// # Errors
///
/// Returns an error if `transfer_type` is not "file" or "folder".
pub fn generate_webrtc_code(
    sender_pubkey: String,
    transfer_id: String,
    relays: Vec<String>,
    filename: String,
    transfer_type: &str,
) -> Result<String> {
    // Validate transfer_type early to fail fast
    if transfer_type != "file" && transfer_type != "folder" {
        anyhow::bail!(
            "Invalid transfer_type: '{}' (expected 'file' or 'folder')",
            transfer_type
        );
    }

    // Validate sender_pubkey format (Nostr x-only Schnorr pubkey: 32 bytes = 64 hex chars)
    if sender_pubkey.len() != 64 || !sender_pubkey.chars().all(|c| c.is_ascii_hexdigit()) {
        anyhow::bail!(
            "Invalid sender_pubkey: expected 64-character hex string (32-byte Nostr pubkey), got {} chars",
            sender_pubkey.len()
        );
    }

    // Validate transfer_id is non-empty
    if transfer_id.trim().is_empty() {
        anyhow::bail!("Invalid transfer_id: cannot be empty");
    }

    // Validate filename is non-empty and doesn't contain path separators
    if filename.trim().is_empty() {
        anyhow::bail!("Invalid filename: cannot be empty");
    }
    if filename.contains('/') || filename.contains('\\') {
        anyhow::bail!("Invalid filename: cannot contain path separators");
    }

    if relays.is_empty() {
        anyhow::bail!("Invalid relays: list cannot be empty");
    }
    for relay in &relays {
        if !relay.starts_with("ws://") && !relay.starts_with("wss://") {
            anyhow::bail!(
                "Invalid relay URL '{}': must start with ws:// or wss://",
                relay
            );
        }
    }

    let token = BeamToken {
        version: CURRENT_VERSION,
        created_at: current_timestamp(),
        sender_pubkey,
        transfer_id,
        relays,
        transfer_type: transfer_type.to_string(),
        filename,
    };

    let serialized = serde_json::to_vec(&token).context("Failed to serialize beam token")?;

    Ok(URL_SAFE_NO_PAD.encode(&serialized))
}

/// Validate beam code format without fully parsing it.
/// Performs lightweight checks (empty, invalid characters, minimum length)
/// without decoding. Returns Ok(()) if the format looks valid.
pub fn validate_code_format(code: &str) -> Result<()> {
    let code = code.trim();

    if code.is_empty() {
        anyhow::bail!("Beam code cannot be empty");
    }

    // Check for invalid characters (base64 URL-safe uses A-Z, a-z, 0-9, -, _)
    // Note: no padding (=) in URL_SAFE_NO_PAD
    if !code
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        anyhow::bail!(
            "Invalid beam code: contains invalid characters. Expected base64url-encoded string."
        );
    }

    // Minimum length check: minimal token data
    if code.len() < MIN_CODE_LENGTH {
        anyhow::bail!("Invalid beam code: too short. Make sure you copied the entire code.");
    }

    Ok(())
}

/// Parse a beam code to extract the token
/// Returns a BeamToken containing all transfer metadata
pub fn parse_code(code: &str) -> Result<BeamToken> {
    // Validate format first for better error messages
    validate_code_format(code)?;

    let serialized = URL_SAFE_NO_PAD
        .decode(code.trim())
        .context("Invalid beam code: not valid base64url encoding")?;

    if serialized.len() < 10 {
        anyhow::bail!("Invalid beam code: decoded data too short");
    }

    let token: BeamToken = serde_json::from_slice(&serialized)
        .context("Invalid beam code: failed to parse token. Make sure the code is correct.")?;

    // Validate version
    if token.version != CURRENT_VERSION {
        anyhow::bail!(
            "Unsupported token version {}. This receiver requires version {}.",
            token.version,
            CURRENT_VERSION
        );
    }

    // Validate TTL
    let now = current_timestamp();
    if token.created_at > now + 60 {
        // Allow 60s clock skew into future
        anyhow::bail!("Invalid token: created_at is in the future. Check system clock.");
    }
    let age = now.saturating_sub(token.created_at);
    if age > SESSION_TTL_SECS {
        let minutes = age / 60;
        anyhow::bail!(
            "Token expired: code is {} minutes old (max {} minutes). \
             Please request a new code from the sender.",
            minutes,
            SESSION_TTL_SECS / 60
        );
    }

    if token.sender_pubkey.len() != 64
        || !token.sender_pubkey.chars().all(|c| c.is_ascii_hexdigit())
    {
        anyhow::bail!(
            "Invalid token: sender_pubkey must be a 64-character hex string"
        );
    }
    if token.transfer_id.trim().is_empty() {
        anyhow::bail!("Invalid token: missing transfer ID");
    }
    if token.filename.trim().is_empty() {
        anyhow::bail!("Invalid token: missing filename");
    }
    if token.relays.is_empty() {
        anyhow::bail!("Invalid token: missing relay list");
    }
    for relay in &token.relays {
        if !relay.starts_with("ws://") && !relay.starts_with("wss://") {
            anyhow::bail!(
                "Invalid token: relay URL '{}' must start with ws:// or wss://",
                relay
            );
        }
    }
    match token.transfer_type.as_str() {
        "file" | "folder" => {}
        invalid => {
            anyhow::bail!(
                "Invalid token: unsupported transfer type '{}' (expected 'file' or 'folder')",
                invalid
            );
        }
    }

    Ok(token)
}
