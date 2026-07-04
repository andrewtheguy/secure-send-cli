# xfer-webrtc

CLI companion for `secure-send-web`.

This project is pre-release software. No backward compatibility or legacy
protocol support is maintained.

## What It Does

`xfer-webrtc` sends and receives single files with the same wire formats as
`secure-send-web`:

- Nostr PIN signaling by default, compatible with the web app's Auto Exchange mode.
- Manual SS03 copy/paste signaling with `--manual`, compatible with the web app's manual exchange codes.
- WebRTC data-channel transfer using the web app's encrypted chunk protocol.
- No QR code support in the CLI.

The file bytes flow over the WebRTC data channel. Nostr relays carry only
encrypted metadata and WebRTC signaling events.

## Install

```bash
cargo build --release --all-features
```

## Usage

### Nostr PIN Mode

Sender:

```bash
xfer-webrtc send /path/to/file
```

The sender prints a 12-character PIN. Enter that PIN in `secure-send-web` or in
another CLI receiver:

```bash
xfer-webrtc receive <PIN>
```

To choose an output directory:

```bash
xfer-webrtc receive <PIN> --output /path/to/dir
```

### Manual SS03 Mode

Sender:

```bash
xfer-webrtc send --manual /path/to/file
```

Receiver:

```bash
xfer-webrtc receive --manual
```

The sender prints an offer code. The receiver pastes that offer and prints a
response code. The sender pastes the response, then the WebRTC transfer starts.

## Protocol Compatibility

The CLI follows `secure-send-web` as the source of truth:

- PIN metadata event: Nostr kind `24243`.
- ACK and WebRTC signal events: Nostr kind `24242`.
- Default relays match `secure-send-web`.
- PIN-derived keys use PBKDF2-SHA256 with the same labels for metadata, signals,
  and P2P content.
- Manual signaling uses SS03 payloads.
- File chunks use AES-256-GCM with the 2-byte chunk index as AAD, followed by
  `DONE:<count>` and receiver `ACK`.

## Limits

- Single-file transfers only.
- Maximum file size is 100 MiB, matching `secure-send-web`.
- No resume support.
- No QR support.
- No custom relay/discovery mode.

## Development

Run checks with all features:

```bash
cargo test --all-features
cargo clippy --all-features
```

Do not run `cargo fmt` for this repo.
