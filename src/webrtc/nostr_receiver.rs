//! Nostr Auto Exchange receiver compatible with secure-send-web.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use nostr_sdk::prelude::*;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;

use crate::crypto::aes;
use crate::crypto::chunk::MAX_MESSAGE_SIZE;
use crate::crypto::pin::{
    TRANSFER_EXPIRATION_MS, compute_pin_hint, derive_nostr_transfer_keys, is_valid_pin, now_ms,
};
use crate::signaling::nostr::{
    CandidatePayload, NostrClient, PinExchangeEvent, PinExchangePayload, Signal,
    create_authenticated_ack_event, create_signal_event, parse_pin_exchange_event,
    parse_signal_event, pin_exchange_filter, signal_filter_from_sender,
};
use crate::transfer::run_receiver;
use crate::ui;
use crate::util::{format_bytes, resolve_destination};
use crate::webrtc::common::{DcMessenger, WebRtcPeer, open_and_detach};
use crate::webrtc::{add_ice_candidate_safely, advertise_max_message_size, candidate_strings};

const CONNECTION_TIMEOUT: Duration = Duration::from_secs(30);
const WAIT_FOR_SIGNAL_TIMEOUT: Duration = Duration::from_secs(60 * 60);
const ICE_GATHER_TIMEOUT: Duration = Duration::from_secs(5);
const ANSWER_RETRY_INTERVAL: Duration = Duration::from_secs(5);

pub async fn receive_file_nostr(pin: &str, output_dir: Option<PathBuf>) -> Result<()> {
    let pin = pin.trim();
    if !is_valid_pin(pin) {
        bail!("Invalid PIN");
    }

    let step = Instant::now();
    ui::status("Connecting to Nostr relays...");
    let client = NostrClient::connect(Keys::generate()).await?;
    ui::status_timed("Connected to Nostr relays", step.elapsed());

    ui::status("Searching for sender...");
    let exchange = find_exchange_event(&client, pin).await?;

    let file_name = exchange.payload.file_name.clone();
    let file_size = exchange.payload.file_size;
    let mime_type = exchange.payload.mime_type.clone();

    if file_size == 0 {
        bail!("Transfer describes an empty file");
    }
    if file_size > MAX_MESSAGE_SIZE {
        bail!(
            "Transfer is {:.0} MB, which exceeds the {} MB limit",
            file_size as f64 / 1024.0 / 1024.0,
            MAX_MESSAGE_SIZE / 1024 / 1024
        );
    }

    ui::status(&format!(
        "Incoming file: \"{}\" ({}, {})",
        file_name,
        format_bytes(file_size),
        mime_type
    ));
    let Some(dest) = resolve_destination(output_dir, &file_name)? else {
        ui::status("Cancelled.");
        client.disconnect().await;
        return Ok(());
    };

    let ack = create_authenticated_ack_event(
        &client,
        &exchange.sender_pubkey,
        &exchange.transfer_id,
        0,
        &exchange.keys.signals,
        Some(&exchange.matched_hint),
    )?;
    let step = Instant::now();
    ui::status("Publishing receiver ready ACK to Nostr...");
    client.publish(&ack).await?;
    ui::status_timed("Published receiver ready ACK to Nostr", step.elapsed());

    ui::status("Waiting for sender P2P offer...");
    let sender_pubkey = exchange.sender_pubkey;
    let signal_filter = signal_filter_from_sender(&exchange.transfer_id, sender_pubkey);
    let mut notifications = client.notifications();
    let sub_id = client.subscribe(signal_filter.clone()).await?;
    let mut seen = HashSet::new();
    let mut queued_candidates = Vec::new();

    let mut offer_sdp = None;
    for event in client.fetch(signal_filter.clone()).await? {
        handle_pre_offer_signal(
            &event,
            &mut seen,
            &exchange.keys.signals,
            &exchange.transfer_id,
            sender_pubkey,
            &mut offer_sdp,
            &mut queued_candidates,
        )?;
        if offer_sdp.is_some() {
            break;
        }
    }

    tokio::time::timeout(WAIT_FOR_SIGNAL_TIMEOUT, async {
        while offer_sdp.is_none() {
            let event = next_event(&mut notifications).await?;
            handle_pre_offer_signal(
                &event,
                &mut seen,
                &exchange.keys.signals,
                &exchange.transfer_id,
                sender_pubkey,
                &mut offer_sdp,
                &mut queued_candidates,
            )?;
        }
        Ok::<(), anyhow::Error>(())
    })
    .await
    .map_err(|_| anyhow::anyhow!("Timed out waiting for sender offer"))??;

    let offer_sdp = offer_sdp.context("missing sender offer")?;

    ui::status("Creating P2P answer...");
    let mut peer = WebRtcPeer::new().await?;
    let mut data_channel_rx = peer
        .take_data_channel_rx()
        .context("Data channel receiver already taken")?;

    let offer = RTCSessionDescription::offer(offer_sdp).context("Invalid offer SDP")?;
    peer.set_remote_description(offer).await?;
    for candidate in queued_candidates.drain(..) {
        add_ice_candidate_safely(&peer, &candidate).await;
    }

    let answer = peer.create_answer().await?;
    peer.set_local_description(answer.clone()).await?;

    ui::status("Gathering network candidates...");
    let candidates = peer.gather_ice_candidates(ICE_GATHER_TIMEOUT).await?;
    let answer_sdp = advertise_max_message_size(answer.sdp);
    let candidates = candidate_strings(candidates)?;

    let step = Instant::now();
    ui::status("Publishing P2P answer to Nostr...");
    publish_answer_and_candidates(
        &client,
        &sender_pubkey,
        &exchange.transfer_id,
        &answer_sdp,
        &candidates,
        &exchange.keys.signals,
    )
    .await?;
    ui::status_timed("Published P2P answer to Nostr", step.elapsed());

    let peer = Arc::new(peer);
    let mut answer_retry = tokio::time::interval(ANSWER_RETRY_INTERVAL);
    answer_retry.tick().await;

    ui::status("Waiting for data channel...");
    let data_channel_timeout = tokio::time::sleep(CONNECTION_TIMEOUT);
    tokio::pin!(data_channel_timeout);
    let data_channel = loop {
        tokio::select! {
            _ = answer_retry.tick() => {
                let step = Instant::now();
                ui::status("Republishing P2P answer to Nostr...");
                publish_answer_and_candidates(
                    &client,
                    &sender_pubkey,
                    &exchange.transfer_id,
                    &answer_sdp,
                    &candidates,
                    &exchange.keys.signals,
                ).await?;
                ui::status_timed("Republished P2P answer to Nostr", step.elapsed());
            }
            maybe_channel = data_channel_rx.recv() => {
                break maybe_channel.context("Sender never opened a data channel")?;
            }
            event = next_event(&mut notifications) => {
                let event = event?;
                handle_receiver_candidate(
                    &event,
                    &mut seen,
                    &peer,
                    &exchange.keys.signals,
                    &exchange.transfer_id,
                    sender_pubkey,
                ).await?;
            }
            _ = &mut data_channel_timeout => {
                bail!("Timed out waiting for data channel");
            }
        }
    };

    let open = open_and_detach(data_channel, CONNECTION_TIMEOUT);
    tokio::pin!(open);
    let raw = loop {
        tokio::select! {
            result = &mut open => break result?,
            _ = answer_retry.tick() => {
                let step = Instant::now();
                ui::status("Republishing P2P answer to Nostr...");
                publish_answer_and_candidates(
                    &client,
                    &sender_pubkey,
                    &exchange.transfer_id,
                    &answer_sdp,
                    &candidates,
                    &exchange.keys.signals,
                ).await?;
                ui::status_timed("Republished P2P answer to Nostr", step.elapsed());
            }
            event = next_event(&mut notifications) => {
                let event = event?;
                handle_receiver_candidate(
                    &event,
                    &mut seen,
                    &peer,
                    &exchange.keys.signals,
                    &exchange.transfer_id,
                    sender_pubkey,
                ).await?;
            }
        }
    };
    client.unsubscribe(&sub_id).await;

    let info = peer.get_connection_info().await;
    ui::status(&format!("Connected via {}", info.connection_type));

    let mut messenger = DcMessenger::new(raw);
    let result = run_receiver(&mut messenger, &exchange.keys.p2p_content, &dest, file_size).await;

    tokio::time::sleep(Duration::from_millis(200)).await;
    let _ = peer.close().await;
    client.disconnect().await;

    result?;
    ui::status(&format!("Saved to {}", dest.display()));
    Ok(())
}

async fn find_exchange_event(client: &NostrClient, pin: &str) -> Result<PinExchangeEvent> {
    let step = Instant::now();
    ui::status("Deriving PIN lookup hints...");
    let hints = vec![compute_pin_hint(pin, 0), compute_pin_hint(pin, 1)];
    ui::status_timed("Derived PIN lookup hints", step.elapsed());

    let step = Instant::now();
    ui::status("Fetching PIN exchange events from Nostr...");
    let mut events = client.fetch(pin_exchange_filter(&hints)).await?;
    ui::status_timed(
        &format!("Fetched {} candidate PIN exchange event(s)", events.len()),
        step.elapsed(),
    );
    events.sort_by_key(|event| std::cmp::Reverse(event.created_at.as_secs()));

    if !events.is_empty() {
        ui::status("Decrypting candidate transfer metadata...");
    }
    let decrypt_start = Instant::now();
    let mut saw_expired = false;
    let mut candidates_checked = 0usize;
    for event in events {
        let created_at_ms = event.created_at.as_secs() * 1000;
        if now_ms().saturating_sub(created_at_ms) > TRANSFER_EXPIRATION_MS {
            saw_expired = true;
            continue;
        }

        let Some((matched_hint, salt, transfer_id, encrypted_payload)) =
            parse_pin_exchange_event(&event)
        else {
            continue;
        };
        candidates_checked += 1;

        let Ok(keys) = derive_nostr_transfer_keys(pin, &salt) else {
            continue;
        };
        let Ok(decrypted) = aes::decrypt(&keys.metadata, &encrypted_payload) else {
            continue;
        };
        let Ok(payload) = serde_json::from_slice::<PinExchangePayload>(&decrypted) else {
            continue;
        };
        if payload.transfer_id != transfer_id {
            continue;
        }

        ui::status_timed(
            &format!("Matched sender after {candidates_checked} candidate event(s)"),
            decrypt_start.elapsed(),
        );
        return Ok(PinExchangeEvent {
            payload,
            salt,
            transfer_id,
            sender_pubkey: event.pubkey,
            matched_hint,
            created_at_ms,
            keys,
        });
    }

    if saw_expired {
        bail!("Transfer expired. Ask sender to start a new transfer.");
    }
    bail!("No transfer found for this PIN");
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

async fn publish_answer_and_candidates(
    client: &NostrClient,
    sender_pubkey: &PublicKey,
    transfer_id: &str,
    answer_sdp: &str,
    candidates: &[String],
    key: &[u8; aes::AES_KEY_LEN],
) -> Result<()> {
    publish_signal(
        client,
        sender_pubkey,
        transfer_id,
        Signal::Answer {
            sdp: answer_sdp.to_string(),
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

fn handle_pre_offer_signal(
    event: &Event,
    seen: &mut HashSet<EventId>,
    key: &[u8; aes::AES_KEY_LEN],
    transfer_id: &str,
    sender_pubkey: PublicKey,
    offer_sdp: &mut Option<String>,
    queued_candidates: &mut Vec<String>,
) -> Result<()> {
    if !seen.insert(event.id) {
        return Ok(());
    }
    let Some(parsed) = parse_signal_event(event, key, transfer_id) else {
        return Ok(());
    };
    if parsed.pubkey != sender_pubkey {
        return Ok(());
    }

    match parsed.signal {
        Signal::Offer { sdp } => {
            *offer_sdp = Some(sdp);
        }
        Signal::Candidate {
            candidate: Some(candidate),
        } => queued_candidates.push(candidate.candidate),
        _ => {}
    }
    Ok(())
}

async fn handle_receiver_candidate(
    event: &Event,
    seen: &mut HashSet<EventId>,
    peer: &Arc<WebRtcPeer>,
    key: &[u8; aes::AES_KEY_LEN],
    transfer_id: &str,
    sender_pubkey: PublicKey,
) -> Result<()> {
    if !seen.insert(event.id) {
        return Ok(());
    }
    let Some(parsed) = parse_signal_event(event, key, transfer_id) else {
        return Ok(());
    };
    if parsed.pubkey != sender_pubkey {
        return Ok(());
    }

    if let Signal::Candidate {
        candidate: Some(candidate),
    } = parsed.signal
    {
        add_ice_candidate_safely(peer, &candidate.candidate).await;
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
