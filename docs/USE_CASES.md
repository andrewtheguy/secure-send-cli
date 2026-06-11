# Common Use Cases & Scenarios

This guide describes common scenarios where `beam-rs-webrtc` shines and which
mode to use for each.

## 1. Standard Internet Transfer (Nostr signaling)
**Scenario**: You want to send a file to a peer over the internet without
exchanging IP addresses manually.

**Solution**: **Online Mode** (default)
- **Why**: Nostr relays handle signaling so the two peers can negotiate a direct
  WebRTC data channel. STUN provides NAT traversal. Relays are auto-discovered.
- **Command**:
  ```bash
  # Sender
  beam-rs-webrtc send /path/to/file

  # Receiver
  beam-rs-webrtc receive <BEAM_CODE>
  ```
- **Experience**: Share the printed beam code via any channel (chat, paper,
  verbal). The Nostr relays only carry signaling; file bytes flow directly
  peer-to-peer.

---

## 2. No Internet / Relays Blocked (LAN or routed private network)
**Scenario**: You need to transfer files when Nostr relays are unavailable (no
internet, or relays blocked), but both machines can still reach each other
directly over a LAN or routed private/VPN network.

**Solution**: **Manual Mode** (`send --manual` / `receive`)
- **Why**: Signaling is exchanged by copy-paste instead of through a relay, so no
  internet or third-party service is required. The data channel is still a direct
  peer-to-peer WebRTC connection.
- **Command**:
  ```bash
  # Sender
  beam-rs-webrtc send --manual /path/to/file

  # Receiver (paste the manual offer; the mode is detected automatically)
  beam-rs-webrtc receive
  ```
- **Experience**: The sender prints an offer code; the receiver pastes it into
  `receive` (which auto-detects manual mode) and replies with an answer code. The
  exchanged text includes signaling metadata and the encryption key, so use a
  secure channel (SSH, remote desktop, encrypted chat).

---

## 3. Self-Hosted Signaling (Custom Nostr relays)
**Scenario**: You require control over the signaling infrastructure and cannot
rely on auto-discovered public relays due to policy or privacy concerns.

**Solution**: **Custom Relays**
- **Why**: Point both sides at your own Nostr relay(s). The relays only ever see
  signaling traffic, never decrypted content or the content-encryption key.
- **Command**:
  ```bash
  beam-rs-webrtc send --relay wss://my-relay.example.com /path/to/file
  ```
  Repeat `--relay` to list multiple relays.

---

## 4. Folder Transfer
**Scenario**: Sending an entire directory rather than a single file.

**Solution**: Pass the directory path; it is auto-detected and archived (tar)
before transfer.
```bash
beam-rs-webrtc send /path/to/folder
```
