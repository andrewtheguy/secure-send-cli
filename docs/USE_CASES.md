# Common Use Cases & Scenarios

This guide describes common scenarios where `xfer-webrtc` shines and which
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
  xfer-webrtc send /path/to/file

  # Receiver
  xfer-webrtc receive <XFER_CODE>
  ```
- **Experience**: Share the printed xfer code via any channel (chat, paper,
  verbal). The Nostr relays only carry signaling; file bytes flow directly
  peer-to-peer. If the sender cannot complete online signaling, it prompts to
  fall back to manual copy-paste signaling for the same transfer.

---

## 2. No Internet / Relays Blocked (LAN or routed private network)
**Scenario**: You need to transfer files when Nostr relays are unavailable (no
internet, or relays blocked), but both machines can still reach each other
directly over a LAN or routed private/VPN network.

**Solution**: **Manual Mode** (`send --manual` on the sender, plain `receive`
on the receiver)
- **Why**: Signaling is exchanged by copy-paste instead of through a relay, so no
  relay or third-party signaling service is required. The data channel is still a
  direct peer-to-peer WebRTC connection.
- **Note**: Manual mode only removes *relay signaling*. The CLI still creates
  peers with public STUN servers, so ICE will attempt to contact them for
  reflexive candidates if the network allows it. If outbound STUN is blocked,
  direct host candidates may still work on reachable LAN/private networks, but
  there is currently no CLI option that disables STUN.
- **Command**:
  ```bash
  # Sender
  xfer-webrtc send --manual /path/to/file

  # Receiver (paste the manual offer; the mode is detected automatically)
  xfer-webrtc receive
  ```
- **Experience**: The sender prints an offer code; the receiver runs plain
  `xfer-webrtc receive`, pastes the offer, and replies with an answer code. The
  exchanged text includes signaling metadata needed to establish the WebRTC
  data channel.
