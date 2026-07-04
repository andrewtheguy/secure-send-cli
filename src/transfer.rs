//! Message-oriented file transfer over a WebRTC data channel, matching
//! secure-send-web's manual-mode choreography:
//!
//! - Sender: send each 128 KiB plaintext chunk as an encrypted binary message
//!   (index 0..N-1), then the text message `DONE:N`, then await the text `ACK`.
//! - Receiver: validate + decrypt each chunk to its `index * 128KiB` offset,
//!   and once `DONE:N` confirms the count, reply with `ACK`.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use bytes::Bytes;
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

use crate::crypto::chunk::{
    ENCRYPTION_CHUNK_SIZE, NONCE_LEN, TAG_LEN, decrypt_chunk, encrypt_chunk, parse_chunk_message,
};
use crate::ui::{self, Direction};
use crate::webrtc::common::DcMessenger;

/// How long the sender waits for the receiver's `ACK` after `DONE`.
const ACK_TIMEOUT: Duration = Duration::from_secs(30);
/// Overall cap on the receive loop, matching the web app.
const RECEIVE_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// SCTP send-buffer high-water mark for backpressure (matches web's 1 MiB).
const MAX_BUFFERED: usize = 1024 * 1024;
/// The chunk index is a 2-byte big-endian field, so valid totals are 0..=65536.
const MAX_CHUNKS: u64 = 0x10000;

/// Number of 128 KiB chunks needed for `total_bytes` (files are non-empty).
fn chunk_count(total_bytes: u64) -> u64 {
    total_bytes.div_ceil(ENCRYPTION_CHUNK_SIZE as u64)
}

/// Plaintext length of chunk `index` given the total size.
fn plaintext_len(index: u64, total_bytes: u64) -> usize {
    let start = index * ENCRYPTION_CHUNK_SIZE as u64;
    (total_bytes - start).min(ENCRYPTION_CHUNK_SIZE as u64) as usize
}

/// Read up to `buf.len()` bytes, returning the number read (short only at EOF).
async fn read_full(file: &mut File, buf: &mut [u8]) -> Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        let n = file.read(&mut buf[filled..]).await?;
        if n == 0 {
            break;
        }
        filled += n;
    }
    Ok(filled)
}

/// Send `file` (of length `total_bytes`) encrypted with `key` over `messenger`.
pub async fn run_sender(
    messenger: &mut DcMessenger,
    key: &[u8; 32],
    file: &mut File,
    total_bytes: u64,
) -> Result<()> {
    let total_chunks = chunk_count(total_bytes);
    if total_chunks > MAX_CHUNKS {
        bail!("file too large: {total_chunks} chunks exceeds protocol limit of {MAX_CHUNKS}");
    }

    let mut buf = vec![0u8; ENCRYPTION_CHUNK_SIZE];
    let mut index: u16 = 0;
    let mut chunks_sent: u64 = 0;
    let mut sent: u64 = 0;

    loop {
        let n = read_full(file, &mut buf).await?;
        if n == 0 {
            break;
        }

        let encrypted = encrypt_chunk(key, &buf[..n], index)?;

        // Backpressure: don't outrun the SCTP send buffer.
        while messenger.buffered_amount() > MAX_BUFFERED {
            if messenger.is_closed() {
                bail!("data channel closed during transfer");
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        messenger.send_binary(Bytes::from(encrypted)).await?;

        sent += n as u64;
        chunks_sent += 1;
        ui::progress(Direction::Send, sent, total_bytes);

        if sent == total_bytes {
            break;
        }
        index = index
            .checked_add(1)
            .context("chunk index exceeded protocol range")?;
    }
    ui::progress_end();

    if chunks_sent != total_chunks {
        bail!(
            "internal error: sent {chunks_sent} chunks but expected {total_chunks} for {sent} bytes"
        );
    }

    // Drain the send buffer so DONE arrives after every chunk.
    while messenger.buffered_amount() > 0 {
        if messenger.is_closed() {
            bail!("data channel closed before completion");
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    messenger.send_text(format!("DONE:{total_chunks}")).await?;
    ui::status("Waiting for receiver acknowledgment...");

    wait_for_ack(messenger).await
}

async fn wait_for_ack(messenger: &mut DcMessenger) -> Result<()> {
    let recv_ack = async {
        loop {
            match messenger.recv().await {
                Some(msg) if msg.is_string => {
                    if msg.data.as_ref() == b"ACK" {
                        return Ok(());
                    }
                    // Ignore any other control strings.
                }
                Some(_) => {} // Ignore stray binary messages.
                None => bail!("data channel closed before acknowledgment"),
            }
        }
    };

    tokio::time::timeout(ACK_TIMEOUT, recv_ack)
        .await
        .map_err(|_| anyhow::anyhow!("timed out waiting for receiver acknowledgment"))?
}

/// Receive a file of `total_bytes` into `dest`, decrypting with `key`.
///
/// Writes to `<dest>.part` and atomically renames on success.
pub async fn run_receiver(
    messenger: &mut DcMessenger,
    key: &[u8; 32],
    dest: &Path,
    total_bytes: u64,
) -> Result<()> {
    let expected_chunks = chunk_count(total_bytes);

    let part_path = dest.with_extension(match dest.extension().and_then(|e| e.to_str()) {
        Some(ext) => format!("{ext}.part"),
        None => "part".to_string(),
    });
    let mut out = File::create(&part_path)
        .await
        .with_context(|| format!("Failed to create {}", part_path.display()))?;
    out.set_len(total_bytes).await?;

    let mut received = vec![false; expected_chunks as usize];
    let mut received_count: u64 = 0;
    let mut received_bytes: u64 = 0;
    let mut done_signalled = false;

    let result = tokio::time::timeout(RECEIVE_TIMEOUT, async {
        while let Some(msg) = messenger.recv().await {
            if msg.is_string {
                let text = String::from_utf8_lossy(&msg.data);
                if let Some(rest) = text.strip_prefix("DONE:") {
                    let n = parse_done_count(rest)
                        .with_context(|| format!("invalid DONE message: {text:?}"))?;
                    if n != expected_chunks {
                        bail!("sender reported {n} chunks, expected {expected_chunks}");
                    }
                    done_signalled = true;
                    break;
                }
                bail!("unexpected control message: {text:?}");
            }

            // Binary message: one encrypted chunk.
            let (index, encrypted) = parse_chunk_message(&msg.data)?;
            let index_u64 = index as u64;
            if index_u64 >= expected_chunks {
                bail!("chunk index {index} out of range (expected < {expected_chunks})");
            }
            if received[index as usize] {
                bail!("duplicate chunk index {index}");
            }

            let expect_plain = plaintext_len(index_u64, total_bytes);
            let expect_encrypted = expect_plain + NONCE_LEN + TAG_LEN;
            if encrypted.len() != expect_encrypted {
                bail!(
                    "chunk {index}: expected {expect_encrypted} encrypted bytes, got {}",
                    encrypted.len()
                );
            }

            let plaintext = decrypt_chunk(key, encrypted, index)?;
            if plaintext.len() != expect_plain {
                bail!(
                    "chunk {index}: expected {expect_plain} plaintext bytes, got {}",
                    plaintext.len()
                );
            }

            let offset = index_u64 * ENCRYPTION_CHUNK_SIZE as u64;
            out.seek(std::io::SeekFrom::Start(offset)).await?;
            out.write_all(&plaintext).await?;

            received[index as usize] = true;
            received_count += 1;
            received_bytes += plaintext.len() as u64;
            ui::progress(Direction::Receive, received_bytes, total_bytes);
        }
        Ok::<(), anyhow::Error>(())
    })
    .await;

    ui::progress_end();

    // Clean up the partial file on any failure path.
    let cleanup = || {
        let _ = std::fs::remove_file(&part_path);
    };

    match result {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            cleanup();
            return Err(e);
        }
        Err(_) => {
            cleanup();
            bail!("transfer timed out after {} seconds", RECEIVE_TIMEOUT.as_secs());
        }
    }

    if !done_signalled {
        cleanup();
        bail!("data channel closed before transfer completed");
    }
    if received_count != expected_chunks || received_bytes != total_bytes {
        cleanup();
        bail!(
            "incomplete transfer: got {received_count}/{expected_chunks} chunks, \
             {received_bytes}/{total_bytes} bytes"
        );
    }

    out.flush().await?;
    out.sync_all().await?;
    drop(out);

    tokio::fs::rename(&part_path, dest)
        .await
        .with_context(|| format!("Failed to move into place: {}", dest.display()))?;

    // Only acknowledge after the file is fully authenticated and persisted.
    messenger.send_text("ACK").await?;
    Ok(())
}

fn parse_done_count(count: &str) -> Result<u64> {
    if count.is_empty() || !count.bytes().all(|byte| byte.is_ascii_digit()) {
        bail!("non-numeric chunk count");
    }
    count.parse().context("chunk count out of range")
}

#[cfg(test)]
mod tests {
    use super::parse_done_count;

    #[test]
    fn done_count_accepts_digits_only() {
        assert_eq!(parse_done_count("0").unwrap(), 0);
        assert_eq!(parse_done_count("42").unwrap(), 42);
        assert!(parse_done_count("").is_err());
        assert!(parse_done_count("42 ").is_err());
        assert!(parse_done_count("42x").is_err());
    }
}
