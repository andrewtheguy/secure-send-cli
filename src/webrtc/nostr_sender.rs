//! Nostr Auto Exchange sender compatible with secure-send-web.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use nostr_sdk::prelude::*;
use tokio::fs::File;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;

use crate::archive::SendSource;
use crate::crypto::aes;
use crate::crypto::chunk::MAX_MESSAGE_SIZE;
use crate::crypto::pin::{
    compute_pin_fingerprint, compute_pin_hint, derive_nostr_transfer_keys, format_pin_fingerprint,
    generate_pin, generate_salt, generate_transfer_id, is_expired, now_ms,
};
use crate::signaling::nostr::{
    self, CandidatePayload, NostrClient, PinExchangePayload, Signal, ack_filter,
    create_pin_exchange_event, create_signal_event, parse_ack_event, parse_signal_event,
    signal_filter_from_author,
};
use crate::transfer::run_sender;
use crate::ui;
use crate::webrtc::common::{DcMessenger, WebRtcPeer, open_and_detach};
use crate::webrtc::{add_ice_candidate_safely, advertise_max_message_size, candidate_strings};

const WAIT_FOR_RECEIVER_TIMEOUT: Duration = Duration::from_secs(60 * 60);
const CONNECTION_TIMEOUT: Duration = Duration::from_secs(30);
const ICE_GATHER_TIMEOUT: Duration = Duration::from_secs(5);
const OFFER_RETRY_INTERVAL: Duration = Duration::from_secs(5);

pub async fn send_file_nostr(source: &SendSource) -> Result<()> {
    let file_size = source.file_size;
    let file_name = source.file_name.clone();
    let mime_type = source.mime_type.to_string();

    if file_size > MAX_MESSAGE_SIZE {
        bail!(
            "File is {:.0} MB, which exceeds the {} MB limit",
            file_size as f64 / 1024.0 / 1024.0,
            MAX_MESSAGE_SIZE / 1024 / 1024
        );
    }

    let session_start = now_ms();
    let step = Instant::now();
    ui::status("Generating secure PIN material...");
    let pin = generate_pin()?;
    let hint = compute_pin_hint(&pin, 0);
    let salt = generate_salt()?;
    let transfer_id = generate_transfer_id()?;
    ui::status_timed("Generated secure PIN material", step.elapsed());

    let step = Instant::now();
    ui::status("Deriving PIN session keys...");
    let keys = derive_nostr_transfer_keys(&pin, &salt)?;
    ui::status_timed("Derived PIN session keys", step.elapsed());

    let step = Instant::now();
    ui::status("Connecting to Nostr relays...");
    let client = NostrClient::connect(Keys::generate()).await?;
    let sender_pubkey = client.public_key();
    ui::status_timed(
        &format!(
            "Connected to Nostr relays ({})",
            nostr::DEFAULT_RELAYS.len()
        ),
        step.elapsed(),
    );

    let step = Instant::now();
    ui::status("Encrypting transfer metadata...");
    let payload = PinExchangePayload {
        content_type: "file".to_string(),
        transfer_id: transfer_id.clone(),
        sender_pubkey: client.public_key_hex(),
        relays: nostr::default_relays_vec(),
        file_name: file_name.clone(),
        file_size,
        mime_type: mime_type.clone(),
    };
    let encrypted_payload = aes::encrypt(&keys.metadata, &serde_json::to_vec(&payload)?)?;
    let exchange_event =
        create_pin_exchange_event(&client, &encrypted_payload, &salt, &transfer_id, &hint)?;
    ui::status_timed("Encrypted transfer metadata", step.elapsed());

    let step = Instant::now();
    ui::status("Publishing PIN exchange to Nostr...");
    client.publish(&exchange_event).await?;
    ui::status_timed("Published PIN exchange to Nostr", step.elapsed());

    ui::show_pin(
        &file_name,
        file_size,
        &pin,
        &format_pin_fingerprint(&compute_pin_fingerprint(&pin)),
    );

    ui::status("Waiting for receiver...");
    let receiver_pubkey =
        wait_for_receiver_ack(&client, &transfer_id, &sender_pubkey, &keys.signals).await?;
    ui::status("Receiver ready ACK received.");

    if is_expired(session_start) {
        bail!("Session expired. Please start a new transfer.");
    }

    ui::status("Creating P2P connection...");
    let mut peer = WebRtcPeer::new().await?;
    let data_channel = peer.create_data_channel("file-transfer").await?;

    let offer = peer.create_offer().await?;
    peer.set_local_description(offer.clone()).await?;

    ui::status("Gathering network candidates...");
    let candidates = peer.gather_ice_candidates(ICE_GATHER_TIMEOUT).await?;
    let offer_sdp = advertise_max_message_size(offer.sdp);
    let candidates = candidate_strings(candidates)?;

    let signal_filter = signal_filter_from_author(&transfer_id, &sender_pubkey, receiver_pubkey);
    let mut notifications = client.notifications();
    let sub_id = client.subscribe(signal_filter.clone()).await?;

    let step = Instant::now();
    ui::status("Publishing P2P offer to Nostr...");
    publish_offer_and_candidates(
        &client,
        &sender_pubkey,
        &transfer_id,
        &offer_sdp,
        &candidates,
        &keys.signals,
    )
    .await?;
    ui::status_timed("Published P2P offer to Nostr", step.elapsed());

    let peer = Arc::new(peer);
    let mut seen = HashSet::new();
    let mut answer_set = false;
    let mut queued_candidates = Vec::new();

    for event in client.fetch(signal_filter.clone()).await? {
        handle_sender_signal(
            &event,
            &mut seen,
            &peer,
            &keys.signals,
            &transfer_id,
            receiver_pubkey,
            &mut answer_set,
            &mut queued_candidates,
        )
        .await?;
    }

    ui::status("Waiting for WebRTC answer...");
    tokio::time::timeout(CONNECTION_TIMEOUT, async {
        let mut retry_interval = tokio::time::interval(OFFER_RETRY_INTERVAL);
        retry_interval.tick().await;

        while !answer_set {
            tokio::select! {
                _ = retry_interval.tick() => {
                    let step = Instant::now();
                    ui::status("Republishing P2P offer to Nostr...");
                    publish_offer_and_candidates(
                        &client,
                        &sender_pubkey,
                        &transfer_id,
                        &offer_sdp,
                        &candidates,
                        &keys.signals,
                    ).await?;
                    ui::status_timed("Republished P2P offer to Nostr", step.elapsed());
                }
                event = next_event(&mut notifications) => {
                    let event = event?;
                    handle_sender_signal(
                        &event,
                        &mut seen,
                        &peer,
                        &keys.signals,
                        &transfer_id,
                        receiver_pubkey,
                        &mut answer_set,
                        &mut queued_candidates,
                    )
                    .await?;
                }
            }
        }
        Ok::<(), anyhow::Error>(())
    })
    .await
    .map_err(|_| anyhow::anyhow!("Timed out waiting for WebRTC answer"))??;

    ui::status("Waiting for data channel...");
    let open = open_and_detach(data_channel, CONNECTION_TIMEOUT);
    tokio::pin!(open);
    let raw = loop {
        tokio::select! {
            result = &mut open => break result?,
            event = next_event(&mut notifications) => {
                let event = event?;
                handle_sender_signal(
                    &event,
                    &mut seen,
                    &peer,
                    &keys.signals,
                    &transfer_id,
                    receiver_pubkey,
                    &mut answer_set,
                    &mut queued_candidates,
                ).await?;
            }
        }
    };
    client.unsubscribe(&sub_id).await;

    let info = peer.get_connection_info().await;
    ui::status(&format!("Connected via {}", info.connection_type));

    let mut messenger = DcMessenger::new(raw);
    let mut file = File::open(&source.path)
        .await
        .with_context(|| format!("Cannot open {}", source.path.display()))?;
    let result = run_sender(&mut messenger, &keys.p2p_content, &mut file, file_size).await;

    let _ = peer.close().await;
    client.disconnect().await;
    result?;

    ui::status("File sent successfully.");
    Ok(())
}

async fn wait_for_receiver_ack(
    client: &NostrClient,
    transfer_id: &str,
    sender_pubkey: &PublicKey,
    key: &[u8; aes::AES_KEY_LEN],
) -> Result<PublicKey> {
    let filter = ack_filter(transfer_id, sender_pubkey);
    let mut notifications = client.notifications();
    let step = Instant::now();
    ui::status("Subscribing for receiver ready ACK...");
    let sub_id = client.subscribe(filter.clone()).await?;
    ui::status_timed("Subscribed for receiver ready ACK", step.elapsed());
    let mut seen = HashSet::new();

    let step = Instant::now();
    ui::status("Checking existing receiver ready ACK events...");
    let events = client.fetch(filter).await?;
    ui::status_timed(
        &format!("Fetched {} existing ACK event(s)", events.len()),
        step.elapsed(),
    );

    for event in events {
        seen.insert(event.id);
        if let Some(ack) = parse_ack_event(&event, key, transfer_id, 0) {
            client.unsubscribe(&sub_id).await;
            return Ok(ack.receiver_pubkey);
        }
    }

    ui::status("Listening for receiver ready ACK...");
    let receiver = tokio::time::timeout(WAIT_FOR_RECEIVER_TIMEOUT, async {
        loop {
            let event = next_event(&mut notifications).await?;
            if !seen.insert(event.id) {
                continue;
            }
            if let Some(ack) = parse_ack_event(&event, key, transfer_id, 0) {
                return Ok::<PublicKey, anyhow::Error>(ack.receiver_pubkey);
            }
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("Timed out waiting for receiver"))??;

    client.unsubscribe(&sub_id).await;
    Ok(receiver)
}

async fn publish_signal(
    client: &NostrClient,
    sender_pubkey: &PublicKey,
    transfer_id: &str,
    signal: Signal,
    key: &[u8; aes::AES_KEY_LEN],
) -> Result<()> {
    let event = create_signal_event(client, sender_pubkey, transfer_id, signal, key)?;
    client.publish(&event).await
}

async fn publish_offer_and_candidates(
    client: &NostrClient,
    sender_pubkey: &PublicKey,
    transfer_id: &str,
    offer_sdp: &str,
    candidates: &[String],
    key: &[u8; aes::AES_KEY_LEN],
) -> Result<()> {
    publish_signal(
        client,
        sender_pubkey,
        transfer_id,
        Signal::Offer {
            sdp: offer_sdp.to_string(),
        },
        key,
    )
    .await?;

    for candidate in candidates {
        publish_signal(
            client,
            sender_pubkey,
            transfer_id,
            Signal::Candidate {
                candidate: Some(CandidatePayload {
                    candidate: candidate.clone(),
                    sdp_mid: Some("0".to_string()),
                    sdp_m_line_index: Some(0),
                }),
            },
            key,
        )
        .await?;
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn handle_sender_signal(
    event: &Event,
    seen: &mut HashSet<EventId>,
    peer: &Arc<WebRtcPeer>,
    key: &[u8; aes::AES_KEY_LEN],
    transfer_id: &str,
    receiver_pubkey: PublicKey,
    answer_set: &mut bool,
    queued_candidates: &mut Vec<String>,
) -> Result<()> {
    if !seen.insert(event.id) {
        return Ok(());
    }
    let Some(parsed) = parse_signal_event(event, key, transfer_id) else {
        return Ok(());
    };
    if parsed.pubkey != receiver_pubkey {
        return Ok(());
    }

    match parsed.signal {
        Signal::Answer { sdp } if !*answer_set => {
            let answer = RTCSessionDescription::answer(sdp).context("Invalid answer SDP")?;
            peer.set_remote_description(answer).await?;
            *answer_set = true;
            for candidate in queued_candidates.drain(..) {
                add_ice_candidate_safely(peer, &candidate).await;
            }
        }
        Signal::Candidate {
            candidate: Some(candidate),
        } => {
            if *answer_set {
                add_ice_candidate_safely(peer, &candidate.candidate).await;
            } else {
                queued_candidates.push(candidate.candidate);
            }
        }
        _ => {}
    }

    Ok(())
}

async fn next_event(
    notifications: &mut tokio::sync::broadcast::Receiver<RelayPoolNotification>,
) -> Result<Event> {
    loop {
        match notifications.recv().await {
            Ok(RelayPoolNotification::Event { event, .. }) => return Ok((*event).clone()),
            Ok(RelayPoolNotification::Message { message, .. }) => {
                if let RelayMessage::Event { event, .. } = message {
                    return Ok((*event).clone());
                }
            }
            Ok(_) => {}
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
            Err(e) => bail!("Nostr notification stream closed: {e}"),
        }
    }
}
