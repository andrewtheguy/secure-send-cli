# beam-rs-webrtc

> [!NOTE]
> This project is still work in progress (0.0.x). No backward compatibility is guaranteed between versions.

A secure, cross-platform, single-binary peer-to-peer file transfer tool built on encrypted WebRTC data channels.

## Features

- **End-to-end encryption** - WebRTC data channels encrypt transfers with DTLS
- **WebRTC data channels** - Direct peer-to-peer connectivity with STUN-based NAT traversal
- **Nostr signaling** - Online connection setup via Nostr relays (with relay auto-discovery)
- **Manual signaling** - Offline copy/paste offer/answer exchange when relays are unavailable
- **Resumable file transfers** - Interrupted file downloads can resume from where they left off
- **File and folder transfers** - Send individual files or entire directories (automatically archived)
- **Cross-platform** - Standalone binary for macOS, Linux, and Windows

## Installation

The release installers fetch a native, standalone executable. You only need the binary in your PATH; no runtime dependencies or package managers are required.

### Quick Install (Linux & macOS)

```bash
curl -sSL https://andrewtheguy.github.io/beam-rs/install.sh | bash
```

By default the installer pulls the latest **stable** release. Use `--prerelease` for the newest prerelease, or pass an explicit tag to pin to a specific build. Examples:

```bash
# Latest prerelease
curl -sSL https://andrewtheguy.github.io/beam-rs/install.sh | bash -s -- --prerelease

# Pin to a specific tag
curl -sSL https://andrewtheguy.github.io/beam-rs/install.sh | bash -s 20251210172710
```

### Quick Install (Windows)

```powershell
irm https://andrewtheguy.github.io/beam-rs/install.ps1 | iex
```

By default the PowerShell installer pulls the latest **stable** release. Use `-PreRelease` for the newest prerelease, or pass an explicit tag to pin to a specific build. Examples (args-only parser):

```powershell
# Latest prerelease
$env:BEAM_INSTALL_ARGS='-PreRelease'; irm https://andrewtheguy.github.io/beam-rs/install.ps1 | iex

# Pin to a specific tag
$env:BEAM_INSTALL_ARGS='20251210172710'; irm https://andrewtheguy.github.io/beam-rs/install.ps1 | iex
```

### From Source

```bash
cargo build --release -p beam-rs-webrtc
```

## Usage

Transfers use a **Beam Code** to establish the connection. The code carries
signaling metadata for the WebRTC session.

### Online (Nostr signaling)

The default mode uses Nostr relays for signaling. Relays are auto-discovered
unless you override them.

```bash
# Send a file
beam-rs-webrtc send /path/to/file

# Send a folder (auto-detected and archived)
beam-rs-webrtc send /path/to/folder

# Use the built-in default relays instead of auto-discovery
beam-rs-webrtc send --default-relays /path/to/file

# Use custom Nostr relay(s) (repeat --relay for multiple)
beam-rs-webrtc send --relay wss://relay1.example.com --relay wss://relay2.example.com /path/to/file
```

Receiving:

```bash
# Receive with the code from the sender
beam-rs-webrtc receive <BEAM_CODE>

# Or prompt for the code interactively
beam-rs-webrtc receive

# Receive into a specific directory
beam-rs-webrtc receive <BEAM_CODE> --output /path/to/dir

# Disable resumable transfers (don't save partial downloads)
beam-rs-webrtc receive <BEAM_CODE> --no-resume
```

### Manual Mode (offline signaling)

Use manual mode when Nostr relays are unavailable and both peers still have
direct network reachability (for example, same LAN or a routed private/VPN
path). Offer/answer codes are exchanged by copy-paste.

```bash
# Sender
beam-rs-webrtc send --manual /path/to/file

# Receiver (auto-detects the manual offer when you paste it)
beam-rs-webrtc receive
```

The receiver uses the same `receive` command for both modes: paste a beam code
for a normal Nostr transfer, or paste a manual offer code and it is detected
automatically.

## Common Use Cases

See [USE_CASES.md](docs/USE_CASES.md) for detailed scenarios.

For protocol details and wire formats, see [ARCHITECTURE.md](docs/ARCHITECTURE.md).

## Security

beam-rs-webrtc relies on WebRTC data channel encryption. Nostr and manual
signaling exchange only connection setup metadata; file bytes flow directly
peer-to-peer over DTLS.

| Signaling | Transfer Encryption | Signaling Payload |
|-----------|---------------------|-------------------|
| Nostr (online) | DTLS (WebRTC) | Transfer ID, sender pubkey, relays, file metadata |
| Manual (offline) | DTLS (WebRTC) | SDP, ICE candidates, file metadata |

Nostr relays are used only for signaling and never see file content. Media flows
directly peer-to-peer over the WebRTC data channel.

For the detailed security model, see [ARCHITECTURE.md](docs/ARCHITECTURE.md#security-model).

## License

MIT
