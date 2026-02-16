use agent_dash_core::paths;
use agent_dash_core::protocol::ClientRequest;
use agent_dash_core::relay::RelayMessage;
use base64::prelude::*;
use crypto_box::aead::AeadCore;
use crypto_box::aead::rand_core::RngCore;
use crypto_box::{SalsaBox, aead::Aead, aead::OsRng};
use futures_util::{SinkExt, StreamExt};
use interprocess::local_socket::{tokio::prelude::*, GenericFilePath};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio_tungstenite::tungstenite::Message;

/// Relay pairing configuration stored on disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelayConfig {
    pub relay_url: String,
    pub channel_id: String,
    pub secret_key_b64: String,
    pub public_key_b64: String,
    pub channel_secret_b64: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phone_public_key_b64: Option<String>,
}

impl RelayConfig {
    pub fn load() -> Result<Self, String> {
        let path = paths::relay_config_path();
        let data = std::fs::read_to_string(&path)
            .map_err(|e| format!("Failed to read {}: {e}", path.display()))?;
        serde_json::from_str(&data)
            .map_err(|e| format!("Invalid relay config: {e}"))
    }

    pub fn save(&self) -> Result<(), String> {
        let path = paths::relay_config_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create config dir: {e}"))?;
        }
        let data = serde_json::to_string_pretty(self)
            .map_err(|e| format!("Failed to serialize config: {e}"))?;
        std::fs::write(&path, data)
            .map_err(|e| format!("Failed to write {}: {e}", path.display()))?;
        Ok(())
    }
}

/// Generate a new relay pairing config with fresh keys.
pub fn generate_pairing(relay_url: &str) -> RelayConfig {
    use crypto_box::SecretKey;

    let secret_key = SecretKey::generate(&mut OsRng);
    let public_key = secret_key.public_key();

    // Generate 32-byte channel secret.
    let mut channel_secret = [0u8; 32];
    OsRng.fill_bytes(&mut channel_secret);

    // Derive channel_id = hex(SHA-256(channel_secret)).
    let channel_id = hex_sha256(&channel_secret);

    RelayConfig {
        relay_url: relay_url.to_string(),
        channel_id,
        secret_key_b64: BASE64_STANDARD.encode(secret_key.to_bytes()),
        public_key_b64: BASE64_STANDARD.encode(public_key.as_bytes()),
        channel_secret_b64: BASE64_STANDARD.encode(channel_secret),
        phone_public_key_b64: None,
    }
}

/// Build a pairing URI for encoding into a QR code.
pub fn pairing_uri(config: &RelayConfig) -> String {
    format!(
        "agentdash://pair?relay={}&secret={}&pk={}",
        urlencoded(&config.relay_url),
        urlencoded(&config.channel_secret_b64),
        urlencoded(&config.public_key_b64),
    )
}

/// Render a QR code to the terminal using Unicode block characters.
pub fn render_qr(data: &str) {
    use qrcode::QrCode;

    let code = match QrCode::new(data) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Failed to generate QR code: {e}");
            return;
        }
    };

    let image = code.render::<char>()
        .quiet_zone(true)
        .module_dimensions(2, 1)
        .build();
    println!("{image}");
}

/// Run the relay connector bridge.
pub async fn run_connector(config: &RelayConfig) -> Result<(), String> {
    let phone_pk_b64 = config.phone_public_key_b64.as_deref()
        .ok_or("No phone public key configured. Run `agent-dash relay pair` first and pair from your phone.")?;

    let phone_pk_bytes = BASE64_STANDARD.decode(phone_pk_b64)
        .map_err(|e| format!("Invalid phone public key: {e}"))?;
    let phone_pk_arr: [u8; 32] = phone_pk_bytes.try_into()
        .map_err(|_| "Phone public key must be 32 bytes")?;
    let phone_pk = crypto_box::PublicKey::from(phone_pk_arr);

    let my_sk_bytes = BASE64_STANDARD.decode(&config.secret_key_b64)
        .map_err(|e| format!("Invalid secret key: {e}"))?;
    let my_sk_arr: [u8; 32] = my_sk_bytes.try_into()
        .map_err(|_| "Secret key must be 32 bytes")?;
    let my_sk = crypto_box::SecretKey::from(my_sk_arr);

    let salsa_box = SalsaBox::new(&phone_pk, &my_sk);

    let mut backoff_ms: u64 = 1000;
    let max_backoff_ms: u64 = 30_000;

    loop {
        eprintln!("Connecting to relay at {}...", config.relay_url);

        match run_bridge(config, &salsa_box).await {
            Ok(()) => {
                eprintln!("Relay connection closed cleanly.");
                backoff_ms = 1000;
            }
            Err(e) => {
                eprintln!("Relay connection error: {e}");
            }
        }

        eprintln!("Reconnecting in {}ms...", backoff_ms);
        tokio::time::sleep(std::time::Duration::from_millis(backoff_ms)).await;
        backoff_ms = (backoff_ms * 2).min(max_backoff_ms);
    }
}

async fn run_bridge(config: &RelayConfig, salsa_box: &SalsaBox) -> Result<(), String> {
    // Connect to daemon.sock.
    let socket_name = paths::client_socket_name();
    let daemon_conn = interprocess::local_socket::tokio::Stream::connect(
        socket_name
            .as_str()
            .to_fs_name::<GenericFilePath>()
            .map_err(|e| format!("Invalid socket path: {e}"))?,
    )
    .await
    .map_err(|e| format!("Failed to connect to daemon: {e}"))?;

    let (daemon_reader, mut daemon_writer) = daemon_conn.split();
    let mut daemon_lines = BufReader::new(daemon_reader).lines();

    // Subscribe to daemon events.
    let subscribe = serde_json::to_string(&ClientRequest::Subscribe).unwrap() + "\n";
    daemon_writer
        .write_all(subscribe.as_bytes())
        .await
        .map_err(|e| format!("Failed to subscribe: {e}"))?;

    // Connect to relay WebSocket.
    let (ws_stream, _) = tokio_tungstenite::connect_async(&config.relay_url)
        .await
        .map_err(|e| format!("WebSocket connect failed: {e}"))?;

    let (mut ws_tx, mut ws_rx) = ws_stream.split();

    // Authenticate with relay.
    let auth = RelayMessage::Auth {
        channel_id: config.channel_id.clone(),
        public_key: config.public_key_b64.clone(),
    };
    ws_tx
        .send(Message::Text(serde_json::to_string(&auth).unwrap().into()))
        .await
        .map_err(|e| format!("Failed to send auth: {e}"))?;

    // Wait for AuthOk.
    let auth_resp = ws_rx
        .next()
        .await
        .ok_or("Relay closed before auth response")?
        .map_err(|e| format!("WebSocket error: {e}"))?;

    if let Message::Text(text) = auth_resp {
        match serde_json::from_str::<RelayMessage>(&*text) {
            Ok(RelayMessage::AuthOk { peer_count }) => {
                eprintln!("Authenticated with relay ({peer_count} peers)");
            }
            Ok(RelayMessage::AuthError { message }) => {
                return Err(format!("Auth rejected: {message}"));
            }
            _ => {
                return Err("Unexpected auth response".into());
            }
        }
    } else {
        return Err("Expected text message for auth response".into());
    }

    eprintln!("Bridge active. Forwarding events...");

    // Main bridging loop.
    loop {
        tokio::select! {
            // Daemon event -> encrypt -> relay.
            line = daemon_lines.next_line() => {
                let line = line.map_err(|e| format!("Daemon read error: {e}"))?;
                let Some(line) = line else {
                    return Err("Daemon connection closed".into());
                };
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }

                // Encrypt the daemon event.
                let nonce = SalsaBox::generate_nonce(&mut OsRng);
                let ciphertext = salsa_box
                    .encrypt(&nonce, trimmed.as_bytes())
                    .map_err(|e| format!("Encryption failed: {e}"))?;

                let msg = RelayMessage::Encrypted {
                    channel_id: config.channel_id.clone(),
                    ciphertext: BASE64_STANDARD.encode(&ciphertext),
                    nonce: BASE64_STANDARD.encode(&nonce),
                };
                ws_tx
                    .send(Message::Text(serde_json::to_string(&msg).unwrap().into()))
                    .await
                    .map_err(|e| format!("WebSocket send error: {e}"))?;
            }

            // Relay message -> decrypt -> daemon.
            msg = ws_rx.next() => {
                let Some(msg) = msg else {
                    return Err("WebSocket closed".into());
                };
                let msg = msg.map_err(|e| format!("WebSocket error: {e}"))?;

                let text = match msg {
                    Message::Text(t) => t,
                    Message::Close(_) => return Ok(()),
                    Message::Ping(data) => {
                        let _ = ws_tx.send(Message::Pong(data)).await;
                        continue;
                    }
                    _ => continue,
                };

                match serde_json::from_str::<RelayMessage>(&*text) {
                    Ok(RelayMessage::Encrypted { ciphertext, nonce, .. }) => {
                        let ct_bytes = BASE64_STANDARD.decode(&ciphertext)
                            .map_err(|e| format!("Invalid ciphertext base64: {e}"))?;
                        let nonce_bytes = BASE64_STANDARD.decode(&nonce)
                            .map_err(|e| format!("Invalid nonce base64: {e}"))?;
                        let nonce_arr: [u8; 24] = nonce_bytes.try_into()
                            .map_err(|_| "Invalid nonce length")?;

                        let plaintext = salsa_box
                            .decrypt((&nonce_arr).into(), &ct_bytes[..])
                            .map_err(|e| format!("Decryption failed: {e}"))?;

                        let mut line = String::from_utf8(plaintext)
                            .map_err(|e| format!("Invalid UTF-8 in decrypted payload: {e}"))?;
                        if !line.ends_with('\n') {
                            line.push('\n');
                        }

                        // Forward the decrypted command to the daemon.
                        daemon_writer
                            .write_all(line.as_bytes())
                            .await
                            .map_err(|e| format!("Daemon write error: {e}"))?;
                    }
                    Ok(RelayMessage::PeerChange { peer_count, .. }) => {
                        eprintln!("Peer count changed: {peer_count}");
                    }
                    _ => {}
                }
            }
        }
    }
}

fn hex_sha256(data: &[u8]) -> String {
    let hash = Sha256::digest(data);
    hash.iter().map(|b| format!("{b:02x}")).collect()
}

fn urlencoded(s: &str) -> String {
    s.replace('%', "%25")
        .replace('&', "%26")
        .replace('=', "%3D")
        .replace('+', "%2B")
        .replace(' ', "%20")
}

/// Show relay connection status.
pub async fn cmd_status() {
    match RelayConfig::load() {
        Ok(config) => {
            println!("Relay URL:   {}", config.relay_url);
            println!("Channel ID:  {}...", &config.channel_id[..16.min(config.channel_id.len())]);
            println!("Public key:  {}...", &config.public_key_b64[..16.min(config.public_key_b64.len())]);
            match config.phone_public_key_b64 {
                Some(pk) => println!("Phone key:   {}...", &pk[..16.min(pk.len())]),
                None => println!("Phone key:   (not paired)"),
            }
        }
        Err(e) => {
            eprintln!("No relay config: {e}");
            eprintln!("Run `agent-dash relay pair <url>` to set up pairing.");
            std::process::exit(1);
        }
    }
}

/// Delete relay pairing config.
pub fn cmd_unpair() {
    let path = paths::relay_config_path();
    if path.exists() {
        match std::fs::remove_file(&path) {
            Ok(()) => println!("Relay config removed: {}", path.display()),
            Err(e) => {
                eprintln!("Failed to remove config: {e}");
                std::process::exit(1);
            }
        }
    } else {
        println!("No relay config found.");
    }
}
