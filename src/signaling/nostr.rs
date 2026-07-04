//! Nostr signaling compatible with secure-send-web's Auto Exchange mode.

use std::time::Duration;

use anyhow::{Context, Result};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use nostr_sdk::prelude::*;
use serde::{Deserialize, Serialize};

use crate::crypto::aes;
use crate::crypto::pin::{NostrTransferKeys, TRANSFER_EXPIRATION_MS, now_sec};

pub const DEFAULT_RELAYS: &[&str] = &[
    "wss://relay.damus.io",
    "wss://nos.lol",
    "wss://relay.primal.net",
    "wss://nostr.rocks",
    "wss://relay.nostr.pub",
    "wss://relay.snort.social",
];

const EVENT_KIND_DATA_TRANSFER: u16 = 24242;
const EVENT_KIND_PIN_EXCHANGE: u16 = 24243;
const RELAY_CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const FETCH_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PinExchangePayload {
    pub content_type: String,
    pub transfer_id: String,
    pub sender_pubkey: String,
    pub relays: Vec<String>,
    pub file_name: String,
    pub file_size: u64,
    pub mime_type: String,
}

#[derive(Debug, Clone)]
pub struct PinExchangeEvent {
    pub payload: PinExchangePayload,
    pub salt: Vec<u8>,
    pub transfer_id: String,
    pub sender_pubkey: PublicKey,
    pub matched_hint: String,
    pub created_at_ms: u64,
    pub keys: NostrTransferKeys,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CandidatePayload {
    pub candidate: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sdp_mid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sdp_m_line_index: Option<u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Signal {
    #[serde(rename = "offer")]
    Offer { sdp: String },
    #[serde(rename = "answer")]
    Answer { sdp: String },
    #[serde(rename = "candidate")]
    Candidate {
        #[serde(skip_serializing_if = "Option::is_none")]
        candidate: Option<CandidatePayload>,
    },
}

#[derive(Debug, Serialize, Deserialize)]
struct SignalEnvelope {
    #[serde(rename = "type")]
    payload_type: String,
    signal: Signal,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AckBody {
    #[serde(rename = "type")]
    payload_type: String,
    transfer_id: String,
    seq: i64,
}

#[derive(Debug, Clone)]
pub struct AckEvent {
    pub receiver_pubkey: PublicKey,
}

#[derive(Debug, Clone)]
pub struct ParsedSignalEvent {
    pub event_id: EventId,
    pub pubkey: PublicKey,
    pub signal: Signal,
}

#[derive(Clone)]
pub struct NostrClient {
    client: Client,
    keys: Keys,
}

impl NostrClient {
    pub async fn connect(keys: Keys) -> Result<Self> {
        let client = Client::new(keys.clone());
        for relay in DEFAULT_RELAYS {
            client
                .add_relay(*relay)
                .await
                .with_context(|| format!("Failed to add relay {relay}"))?;
        }
        client.connect().await;
        client.wait_for_connection(RELAY_CONNECT_TIMEOUT).await;
        Ok(Self { client, keys })
    }

    pub fn public_key(&self) -> PublicKey {
        self.keys.public_key()
    }

    pub fn public_key_hex(&self) -> String {
        self.keys.public_key().to_hex()
    }

    pub async fn publish(&self, event: &Event) -> Result<()> {
        self.client
            .send_event(event)
            .await
            .context("Failed to publish Nostr event")?;
        Ok(())
    }

    pub async fn subscribe(&self, filter: Filter) -> Result<SubscriptionId> {
        Ok(self
            .client
            .subscribe(filter, None)
            .await
            .context("Failed to subscribe to Nostr events")?
            .val)
    }

    pub async fn unsubscribe(&self, id: &SubscriptionId) {
        self.client.unsubscribe(id).await;
    }

    pub fn notifications(&self) -> tokio::sync::broadcast::Receiver<RelayPoolNotification> {
        self.client.notifications()
    }

    pub async fn fetch(&self, filter: Filter) -> Result<Vec<Event>> {
        let events = self
            .client
            .fetch_events(filter, FETCH_TIMEOUT)
            .await
            .context("Failed to fetch Nostr events")?;
        Ok(events.into_iter().collect())
    }

    pub async fn disconnect(&self) {
        self.client.disconnect().await;
    }

    pub fn sign(&self, builder: EventBuilder) -> Result<Event> {
        builder
            .sign_with_keys(&self.keys)
            .context("Failed to sign Nostr event")
    }
}

pub fn data_kind() -> Kind {
    Kind::from_u16(EVENT_KIND_DATA_TRANSFER)
}

pub fn pin_exchange_kind() -> Kind {
    Kind::from_u16(EVENT_KIND_PIN_EXCHANGE)
}

pub fn default_relays_vec() -> Vec<String> {
    DEFAULT_RELAYS.iter().map(|relay| (*relay).to_string()).collect()
}

pub fn create_pin_exchange_event(
    client: &NostrClient,
    encrypted_payload: &[u8],
    salt: &[u8],
    transfer_id: &str,
    hint: &str,
) -> Result<Event> {
    let expiration = now_sec() + (TRANSFER_EXPIRATION_MS / 1000);
    let tags = vec![
        tag("h", hint)?,
        tag("s", STANDARD.encode(salt))?,
        tag("t", transfer_id)?,
        tag("type", "pin_exchange")?,
        tag("expiration", expiration.to_string())?,
    ];

    client.sign(
        EventBuilder::new(pin_exchange_kind(), STANDARD.encode(encrypted_payload)).tags(tags),
    )
}

pub fn parse_pin_exchange_event(event: &Event) -> Option<(String, Vec<u8>, String, Vec<u8>)> {
    if event.kind != pin_exchange_kind() {
        return None;
    }

    let hint = tag_value(event, "h")?.to_string();
    let salt = STANDARD.decode(tag_value(event, "s")?).ok()?;
    let transfer_id = tag_value(event, "t")?.to_string();
    let encrypted_payload = STANDARD.decode(&event.content).ok()?;
    Some((hint, salt, transfer_id, encrypted_payload))
}

pub fn create_authenticated_ack_event(
    client: &NostrClient,
    sender_pubkey: &PublicKey,
    transfer_id: &str,
    seq: i64,
    key: &[u8; aes::AES_KEY_LEN],
    hint: Option<&str>,
) -> Result<Event> {
    let body = AckBody {
        payload_type: "ack".to_string(),
        transfer_id: transfer_id.to_string(),
        seq,
    };
    let encrypted = aes::encrypt(key, &serde_json::to_vec(&body)?)?;

    let mut tags = vec![
        tag("p", sender_pubkey.to_hex())?,
        tag("t", transfer_id)?,
        tag("seq", seq.to_string())?,
        tag("type", "ack")?,
    ];
    if let Some(hint) = hint {
        tags.push(tag("h", hint)?);
    }

    client.sign(EventBuilder::new(data_kind(), STANDARD.encode(encrypted)).tags(tags))
}

pub fn parse_ack_event(
    event: &Event,
    key: &[u8; aes::AES_KEY_LEN],
    expected_transfer_id: &str,
    expected_seq: i64,
) -> Option<AckEvent> {
    if event.kind != data_kind() || tag_value(event, "type")? != "ack" {
        return None;
    }
    if tag_value(event, "t")? != expected_transfer_id {
        return None;
    }
    let seq = tag_value(event, "seq")?.parse::<i64>().ok()?;
    if seq != expected_seq {
        return None;
    }

    let encrypted = STANDARD.decode(&event.content).ok()?;
    let decrypted = aes::decrypt(key, &encrypted).ok()?;
    let body: AckBody = serde_json::from_slice(&decrypted).ok()?;
    if body.payload_type != "ack"
        || body.transfer_id != expected_transfer_id
        || body.seq != expected_seq
    {
        return None;
    }

    Some(AckEvent {
        receiver_pubkey: event.pubkey,
    })
}

pub fn create_signal_event(
    client: &NostrClient,
    sender_pubkey: &PublicKey,
    transfer_id: &str,
    signal: Signal,
    key: &[u8; aes::AES_KEY_LEN],
) -> Result<Event> {
    let envelope = SignalEnvelope {
        payload_type: "signal".to_string(),
        signal,
    };
    let encrypted = aes::encrypt(key, &serde_json::to_vec(&envelope)?)?;
    let tags = vec![
        tag("t", transfer_id)?,
        tag("p", sender_pubkey.to_hex())?,
        tag("type", "signal")?,
    ];

    client.sign(EventBuilder::new(data_kind(), STANDARD.encode(encrypted)).tags(tags))
}

pub fn parse_signal_event(
    event: &Event,
    key: &[u8; aes::AES_KEY_LEN],
    expected_transfer_id: &str,
) -> Option<ParsedSignalEvent> {
    if event.kind != data_kind() || tag_value(event, "type")? != "signal" {
        return None;
    }
    if tag_value(event, "t")? != expected_transfer_id {
        return None;
    }

    let encrypted = STANDARD.decode(&event.content).ok()?;
    let decrypted = aes::decrypt(key, &encrypted).ok()?;
    let envelope: SignalEnvelope = serde_json::from_slice(&decrypted).ok()?;
    if envelope.payload_type != "signal" {
        return None;
    }

    Some(ParsedSignalEvent {
        event_id: event.id,
        pubkey: event.pubkey,
        signal: envelope.signal,
    })
}

pub fn ack_filter(transfer_id: &str, sender_pubkey: &PublicKey) -> Filter {
    Filter::new()
        .kind(data_kind())
        .custom_tag(SingleLetterTag::lowercase(Alphabet::T), transfer_id)
        .custom_tag(SingleLetterTag::lowercase(Alphabet::P), sender_pubkey.to_hex())
}

pub fn pin_exchange_filter(hints: &[String]) -> Filter {
    Filter::new()
        .kind(pin_exchange_kind())
        .custom_tags(SingleLetterTag::lowercase(Alphabet::H), hints.iter().cloned())
        .limit(10)
}

pub fn signal_filter(transfer_id: &str, sender_pubkey: &PublicKey) -> Filter {
    Filter::new()
        .kind(data_kind())
        .custom_tag(SingleLetterTag::lowercase(Alphabet::T), transfer_id)
        .custom_tag(SingleLetterTag::lowercase(Alphabet::P), sender_pubkey.to_hex())
}

pub fn signal_filter_from_author(
    transfer_id: &str,
    sender_pubkey: &PublicKey,
    author: PublicKey,
) -> Filter {
    signal_filter(transfer_id, sender_pubkey).author(author)
}

fn tag(name: &str, value: impl Into<String>) -> Result<Tag> {
    Tag::parse([name.to_string(), value.into()]).context("invalid Nostr tag")
}

fn tag_value<'a>(event: &'a Event, name: &str) -> Option<&'a str> {
    event
        .tags
        .iter()
        .find(|tag| tag.as_slice().first().is_some_and(|k| k == name))
        .and_then(|tag| tag.content())
}
