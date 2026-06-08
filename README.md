# beam-rs

> [!NOTE]
> This project is still work in progress (0.0.x). No backward compatibility is guaranteed between versions.

A secure, cross-platform, single-binary peer-to-peer file transfer tool with direct connectivity and AES-256-GCM end-to-end encryption.

## Features

- **End-to-end encryption** - All transfers use AES-256-GCM encryption
- **Resumable file transfers** - Interrupted file downloads can resume from where they left off
- **File and folder transfers** - Send individual files or entire directories (automatically archived)
- **Multiple transport modes** - iroh (recommended), Tor, WebRTC, and local LAN
- **Local discovery** - mDNS for same-network transfers without internet
- **NAT traversal** - Automatic relay fallback for iroh; STUN for WebRTC
- **Anonymous transfers** - Tor hidden services via `beam-rs-tor` for anonymity
- **Cross-platform** - Standalone binary for macOS, Linux, and Windows

## Installation

The release installers fetch a native, standalone executable. You only need the binary in your PATH; no runtime dependencies or package managers are required.

### Quick Install (Linux & macOS)

```bash
curl -sSL https://andrewtheguy.github.io/beam-rs/install.sh | bash
```

To install the WebRTC binary instead:

```bash
curl -sSL https://andrewtheguy.github.io/beam-rs/install.sh | bash -s -- --webrtc
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

To install the WebRTC binary instead (single line):

```powershell
$env:BEAM_INSTALL_ARGS='-WebRTC'; irm https://andrewtheguy.github.io/beam-rs/install.ps1 | iex
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
# Main binary (iroh transport)
cargo build --release

# Tor binary (separate crate, anonymous transfers)
cargo build --release -p beam-rs-tor

# Local LAN binary (separate crate, mDNS discovery)
cargo build --release -p beam-rs-local

# WebRTC binary (separate crate)
cargo build --release -p beam-rs-webrtc
```

## Usage

### Internet Transfers

Use these modes for transfers over the internet. They all use a **Beam Code** for connection.

#### 1. iroh Mode (Recommended) - `send`
*Direct P2P transport using QUIC/TLS with automatic relay fallback. Most reliable for both small and large files.*

```bash
# Send file
beam-rs send /path/to/file

# Send folder
beam-rs send /path/to/folder --folder
```

##### Custom Iroh Relays
- Default behavior uses iroh's public relay fallback plus direct P2P.
- For self-hosted setups, point both sides at your own DERP relay(s):
    ```bash
    beam-rs send --relay-url https://relay1.example.com /path/to/file
    beam-rs receive --relay-url https://relay1.example.com
    ```
- Multiple `--relay-url` flags are supported for failover.

#### 2. Tor Mode - `beam-rs-tor send`
*Anonymous transfers via Tor hidden services. Use when anonymity is required.*
> Built as a separate binary: `cargo build -p beam-rs-tor`.

```bash
beam-rs-tor send /path/to/file
```

#### 3. WebRTC Mode - `beam-rs-webrtc send`
*WebRTC transfers with Nostr signaling for NAT traversal.*
> Built as a separate binary in this workspace: `cargo build -p beam-rs-webrtc`.

```bash
# Send with default Nostr relays
beam-rs-webrtc send /path/to/file

# Send with custom relay
beam-rs-webrtc send --relay wss://my-relay.com /path/to/file

# Receive with code from sender
beam-rs-webrtc receive <BEAM_CODE>

# Or prompt for code interactively
beam-rs-webrtc receive
```

For copy/paste signaling when Nostr relays are unavailable, see [Manual Mode](#manual-mode).

If WebRTC connection fails (e.g., both peers behind symmetric NAT), try iroh mode which has automatic relay fallback.

#### Receiving (Internet)
`beam-rs receive` receives iroh codes, `beam-rs-tor receive` receives Tor codes, and `beam-rs-webrtc receive` (or `receive-manual`) receives WebRTC codes.

```bash
beam-rs receive
# Or with code directly
beam-rs receive --code <BEAM_CODE>

# Receive using PIN
beam-rs receive --pin
```

---

### Local/Offline Transfers

There are **two** ways to transfer without relying on the public internet:

1) **LAN discovery (recommended when both devices share a network)**
   - Uses mDNS discovery + SPAKE2 PIN
   - Fast, zero copy/paste, no internet required

2) **Manual WebRTC (when mDNS is blocked but peers still have direct network reachability)**
   - Uses WebRTC DataChannels with **manual** offer/answer code exchange
   - Works even when Nostr relays are unavailable (see [Manual Mode](#manual-mode))

> **Note**: Tor mode requires internet access. iroh mode can be air‑gapped when you self‑host the relay and point both sides at it via `--relay-url`; the default public relay requires internet access.

#### LAN discovery (`beam-rs-local`)

Use this mode for transfers on the same network (no internet required). A **PIN** is shown and fed into a SPAKE2 PAKE to derive the AES key (not a beam code).

> Built as a separate binary in this workspace: `cargo build -p beam-rs-local`.

```bash
# Send locally
beam-rs-local send /path/to/file

# Send folder locally
beam-rs-local send /path/to/folder --folder

# Receive locally
beam-rs-local receive
```

### Manual Mode

Use manual mode when Nostr relays are unavailable and both peers still have
direct network reachability (for example, same LAN or routed private/VPN path).

```bash
# Sender
beam-rs-webrtc send-manual /path/to/file

# Receiver
beam-rs-webrtc receive-manual
```

Manual mode exchanges offer/answer codes via copy-paste. The codes contain the
encryption key, so only share them through secure channels (SSH, remote
desktop, encrypted chat).

## Common Use Cases

See [USE_CASES.md](docs/USE_CASES.md) for detailed scenarios including:
- **No Internet** - Air-gapped / Local LAN transfers
- **Restricted Networks** - Firewall/NAT traversal options
- **Anonymity** - Tor mode for anonymous transfers
- **Self-Hosted** - Zero third-party dependency setups

For protocol details and wire formats, see [ARCHITECTURE.md](docs/ARCHITECTURE.md).

## Security

All modes provide end-to-end encryption.
- **Internet Modes (iroh, Tor, WebRTC)**: The **Beam Code** carries the key/address information.
- **Local Mode** (`beam-rs-local`): Uses a 12-character PIN that feeds a SPAKE2 PAKE to derive the AES key (no beam code).

| Mode | Type | Key Exchange | Transport Encryption | Content Encryption |
|------|------|--------------|---------------------|-------------------|
| iroh | Internet | Beam Code | QUIC/TLS 1.3 | AES-256-GCM |
| Tor (`beam-rs-tor`) | Internet | Beam Code | Tor circuits | AES-256-GCM |
| WebRTC | Internet | Beam Code | DTLS (WebRTC) | AES-256-GCM |
| Local | LAN | SPAKE2 (PIN + transfer_id) | None (raw TCP) | AES-256-GCM |

Internet modes use dual-layer encryption (transport + content). Local mode uses single-layer AES-256-GCM over raw TCP, which is sufficient for trusted LANs.

Relay servers (iroh, Tor) never see decrypted content or encryption keys.

For detailed security model, see [ARCHITECTURE.md](docs/ARCHITECTURE.md#security-model).

## License

MIT
