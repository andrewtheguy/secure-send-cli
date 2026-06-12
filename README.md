# xfer-webrtc

> [!NOTE]
> This project is still work in progress (0.0.x). No backward compatibility is guaranteed between versions.

A secure, cross-platform, single-binary peer-to-peer file transfer tool built on encrypted WebRTC data channels.

## Features


## Installation

The release installers fetch a native, standalone executable. You only need the binary in your PATH; no runtime dependencies or package managers are required.

### Quick Install (Linux & macOS)

```bash
curl -sSL https://andrewtheguy.github.io/xfer/install.sh | bash
```

By default the installer pulls the latest **stable** release. Use `--prerelease` for the newest prerelease, or pass an explicit tag to pin to a specific build. Examples:

```bash
# Latest prerelease
curl -sSL https://andrewtheguy.github.io/xfer/install.sh | bash -s -- --prerelease

# Pin to a specific tag
curl -sSL https://andrewtheguy.github.io/xfer/install.sh | bash -s 20251210172710
```

### Quick Install (Windows)

```powershell
irm https://andrewtheguy.github.io/xfer/install.ps1 | iex
```

By default the PowerShell installer pulls the latest **stable** release. Use `-PreRelease` for the newest prerelease, or pass an explicit tag to pin to a specific build. Examples (args-only parser):

```powershell
# Latest prerelease
$env:XFER_INSTALL_ARGS='-PreRelease'; irm https://andrewtheguy.github.io/xfer/install.ps1 | iex

# Pin to a specific tag
$env:XFER_INSTALL_ARGS='20251210172710'; irm https://andrewtheguy.github.io/xfer/install.ps1 | iex
```

### From Source

```bash
cargo build --release
```

## Usage

Transfers use a **Xfer Code** to establish the connection. The code carries
signaling metadata for the WebRTC session.

### Online (Nostr signaling)

The default mode uses Nostr relays for signaling. Relays are auto-discovered
unless you specify custom Nostr relay URLs.

```bash
# Send a file
xfer-webrtc send /path/to/file

# Send a folder (auto-detected and archived)
xfer-webrtc send /path/to/folder

# Use the built-in default relays instead of auto-discovery
xfer-webrtc send --default-relays /path/to/file

# Use a custom Nostr relay URL (repeat --relay for multiple)
xfer-webrtc send --relay wss://relay1.example.com --relay wss://relay2.example.com /path/to/file
```

Receiving:

```bash
# Receive with the code from the sender
xfer-webrtc receive <XFER_CODE>

# Or prompt for the code interactively
xfer-webrtc receive

# Receive into a specific directory
xfer-webrtc receive <XFER_CODE> --output /path/to/dir

# Disable resumable transfers (don't save partial downloads)
xfer-webrtc receive <XFER_CODE> --no-resume
```

### Manual Mode (offline signaling)

Use manual mode when Nostr relays are unavailable and both peers still have
direct network reachability (for example, same LAN or a routed private/VPN
path). Offer/answer codes are exchanged by copy-paste.

```bash
# Sender
xfer-webrtc send --manual /path/to/file

# Receiver (auto-detects the manual offer when you paste it)
xfer-webrtc receive
```

The receiver uses the same `receive` command for both modes: paste a xfer code
for a normal Nostr transfer, or paste a manual offer code and it is detected
automatically.

## Common Use Cases

See [USE_CASES.md](docs/USE_CASES.md) for detailed scenarios.

For protocol details and wire formats, see [ARCHITECTURE.md](docs/ARCHITECTURE.md).

## Security


For the detailed security model, see [ARCHITECTURE.md](docs/ARCHITECTURE.md#security-model).

## License

MIT
