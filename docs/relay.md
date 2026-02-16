# Encrypted WebSocket Relay

Access your Claude Code sessions from a phone by running a cloud relay that bridges `daemon.sock` to a WebSocket endpoint with end-to-end encryption.

## How it works

```
Phone App ──WSS──> ┌─────────────┐ <──WSS── agent-dash relay connect ── daemon.sock
                   │ Relay Server │
                   │ (VPS/cloud)  │
                   └─────────────┘
                    routes by channel_id
                    sees only ciphertext
```

Both the daemon connector and the phone connect outward to the relay server. The relay only ever sees encrypted blobs — it cannot read session data or commands. Security comes from:

- **Channel ID secrecy**: `channel_id = hex(SHA-256(channel_secret))`. Only QR-paired devices know it. 256-bit hash space makes brute-force infeasible.
- **End-to-end encryption**: Payloads are NaCl-encrypted (XSalsa20-Poly1305 via `crypto_box`) with X25519 keys the relay never sees.

## Components

| Component | Where it runs | Binary |
|-----------|--------------|--------|
| Relay server | VPS / cloud | `agent-dash-relay` |
| Daemon connector | Your dev machine | `agent-dash relay connect` |
| Phone app | Phone | Future work |

## Setup

### 1. Deploy the relay server

Build and deploy `agent-dash-relay` on a VPS with a public IP or domain:

```bash
cargo build -p agent-dash-relay --release
scp target/release/agent-dash-relay my-vps:~/
```

On the VPS:

```bash
# Defaults to 0.0.0.0:8443
./agent-dash-relay

# Or customize
./agent-dash-relay --bind 0.0.0.0:9000 --max-buffer 5000 --channel-ttl 172800
```

**Options:**

| Flag | Default | Description |
|------|---------|-------------|
| `--bind` | `0.0.0.0:8443` | Address and port to listen on |
| `--max-buffer` | `1000` | Max buffered messages per channel (ring buffer) |
| `--channel-ttl` | `86400` | Seconds before idle channels with no peers are evicted |

For production, put the relay behind a reverse proxy with TLS (nginx, caddy) so clients connect via `wss://`.

### 2. Pair your phone

On your dev machine (with the daemon running):

```bash
agent-dash relay pair wss://my-relay.example.com
```

This:
1. Generates an X25519 keypair
2. Generates a 32-byte random channel secret
3. Derives `channel_id = hex(SHA-256(channel_secret))`
4. Saves everything to `~/.config/agent-dash/relay.json`
5. Displays a QR code containing a pairing URI

The QR code encodes:

```
agentdash://pair?relay=<url>&secret=<channel_secret_b64>&pk=<daemon_public_key_b64>
```

Scan this with the agent-dash phone app (future). The phone extracts the relay URL, channel secret, and daemon's public key, generates its own keypair, and connects to the relay.

### 3. Connect the daemon to the relay

Once your phone is paired (its public key is stored in `relay.json`):

```bash
agent-dash relay connect
```

This bridges `daemon.sock` and the relay WebSocket:

- Daemon events are encrypted and forwarded to the relay
- Phone commands arrive encrypted from the relay and are decrypted and forwarded to the daemon
- Reconnects automatically with exponential backoff (1s, 2s, 4s, ..., 30s max) on disconnect

### 4. Check status

```bash
agent-dash relay status
```

Shows the configured relay URL, channel ID prefix, and whether a phone public key is paired.

### 5. Unpair

```bash
agent-dash relay unpair
```

Deletes `~/.config/agent-dash/relay.json`, removing all keys and pairing data.

## Configuration

Pairing config is stored at `~/.config/agent-dash/relay.json`:

```json
{
    "relay_url": "wss://my-relay.example.com",
    "channel_id": "a1b2c3d4...",
    "secret_key_b64": "...",
    "public_key_b64": "...",
    "channel_secret_b64": "...",
    "phone_public_key_b64": null
}
```

| Field | Description |
|-------|-------------|
| `relay_url` | WebSocket URL of the relay server |
| `channel_id` | `hex(SHA-256(channel_secret))` — used to join the channel on the relay |
| `secret_key_b64` | Daemon's X25519 secret key (base64) |
| `public_key_b64` | Daemon's X25519 public key (base64) |
| `channel_secret_b64` | Shared channel secret (base64), included in QR code |
| `phone_public_key_b64` | Phone's X25519 public key (base64), set after phone pairs |

## Relay protocol

The relay uses a JSON WebSocket protocol. All messages are tagged with a `type` field.

### Client -> Relay

**Auth** — join a channel:
```json
{"type": "auth", "channel_id": "abc123...", "public_key": "base64..."}
```

**Encrypted** — send an encrypted payload to other peers:
```json
{"type": "encrypted", "channel_id": "abc123...", "ciphertext": "base64...", "nonce": "base64..."}
```

**Sync** — request buffered messages missed while offline:
```json
{"type": "sync", "channel_id": "abc123...", "since_seq": 42}
```

### Relay -> Client

**AuthOk** — authentication accepted:
```json
{"type": "auth_ok", "peer_count": 2}
```

**AuthError** — authentication rejected:
```json
{"type": "auth_error", "message": "reason"}
```

**PeerChange** — a peer joined or left the channel:
```json
{"type": "peer_change", "channel_id": "abc123...", "peer_count": 1}
```

**SyncResponse** — buffered messages:
```json
{"type": "sync_response", "channel_id": "abc123...", "messages": [
    {"seq": 1, "ciphertext": "base64...", "nonce": "base64...", "timestamp": 1707900000}
]}
```

**Encrypted** — forwarded from another peer (same format as above).

## Security model

The relay is a **dumb encrypted-blob router**. It never has access to:
- The channel secret (it only sees the SHA-256 hash as channel_id)
- Any encryption keys (X25519 keypairs stay on endpoints)
- Plaintext payloads (all application data is NaCl-encrypted)

The relay does NOT verify signatures or authenticate clients beyond channel_id matching. This is intentional — security is provided entirely by:

1. **Channel ID secrecy**: Only devices that scanned the QR code know the channel secret from which the channel_id is derived.
2. **End-to-end encryption**: Even if an attacker guesses a channel_id, they cannot decrypt payloads without the X25519 private keys.

## Relay server internals

The relay server follows the same single-owner concurrency pattern as `agent-dashd`:

- One tokio task owns all channel state via an `mpsc` channel
- Connection handlers send `ChannelCmd` messages to the channel manager
- No shared mutexes — all state mutation happens in a single task
- Channels are created on first `Auth` and evicted after `--channel-ttl` seconds of inactivity
- Message buffer is a bounded ring buffer (`VecDeque`) per channel, oldest messages evicted when `--max-buffer` is exceeded
