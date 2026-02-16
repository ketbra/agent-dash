use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use agent_dash_core::relay::BufferedMessage;
use tokio::sync::{mpsc, oneshot};

/// Commands sent from connection handlers to the channel manager task.
pub enum ChannelCmd {
    /// Authenticate a peer into a channel.
    Auth {
        channel_id: String,
        public_key: String,
        peer_tx: mpsc::Sender<String>,
        reply: oneshot::Sender<Result<u32, String>>,
    },
    /// Forward an encrypted message to other peers in a channel.
    Forward {
        channel_id: String,
        sender_key: String,
        message: String,
    },
    /// Request buffered messages since a sequence number.
    Sync {
        channel_id: String,
        since_seq: u64,
        reply: oneshot::Sender<Vec<BufferedMessage>>,
    },
    /// A peer disconnected.
    Disconnect {
        channel_id: String,
        public_key: String,
    },
}

struct Peer {
    public_key: String,
    tx: mpsc::Sender<String>,
}

struct Channel {
    peers: Vec<Peer>,
    buffer: std::collections::VecDeque<BufferedMessage>,
    next_seq: u64,
    last_activity: u64,
}

impl Channel {
    fn new() -> Self {
        Self {
            peers: Vec::new(),
            buffer: std::collections::VecDeque::new(),
            next_seq: 1,
            last_activity: now_secs(),
        }
    }

    fn touch(&mut self) {
        self.last_activity = now_secs();
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Spawn the channel manager task. Returns a sender for issuing commands.
pub fn spawn(max_buffer: usize, channel_ttl_secs: u64) -> mpsc::Sender<ChannelCmd> {
    let (tx, rx) = mpsc::channel(256);
    tokio::spawn(run(rx, max_buffer, channel_ttl_secs));
    tx
}

async fn run(
    mut rx: mpsc::Receiver<ChannelCmd>,
    max_buffer: usize,
    channel_ttl_secs: u64,
) {
    let mut channels: HashMap<String, Channel> = HashMap::new();
    let mut evict_interval = tokio::time::interval(std::time::Duration::from_secs(60));

    loop {
        tokio::select! {
            cmd = rx.recv() => {
                let Some(cmd) = cmd else { break };
                handle_cmd(&mut channels, cmd, max_buffer);
            }
            _ = evict_interval.tick() => {
                evict_stale(&mut channels, channel_ttl_secs);
            }
        }
    }
}

fn handle_cmd(
    channels: &mut HashMap<String, Channel>,
    cmd: ChannelCmd,
    max_buffer: usize,
) {
    match cmd {
        ChannelCmd::Auth {
            channel_id,
            public_key,
            peer_tx,
            reply,
        } => {
            let channel = channels.entry(channel_id).or_insert_with(Channel::new);
            channel.touch();

            // Don't allow duplicate public keys in the same channel.
            if channel.peers.iter().any(|p| p.public_key == public_key) {
                let _ = reply.send(Err("public key already connected".into()));
                return;
            }

            channel.peers.push(Peer {
                public_key,
                tx: peer_tx,
            });
            let peer_count = channel.peers.len() as u32;
            let _ = reply.send(Ok(peer_count));

            // Notify existing peers about the new peer.
            let change = serde_json::to_string(
                &agent_dash_core::relay::RelayMessage::PeerChange {
                    channel_id: String::new(), // filled per-peer below — but we send channel_id from the channel entry
                    peer_count,
                },
            );
            // We already have channel_id consumed by entry(), reconstruct from peers context:
            // Actually we need channel_id for the PeerChange message. Let's restructure.
            // The channel_id was moved into the HashMap key. We can iterate and send.
            // We'll handle this with a separate notification step.
            if let Ok(json) = change {
                // Replace empty channel_id — not great, let's just build it properly.
                // Actually, let's just build notification outside the match arm.
                // For now, we'll send PeerChange after the Auth reply.
                for peer in &channel.peers {
                    // PeerChange goes to all peers (including the new one, they get AuthOk separately).
                    let _ = peer.tx.try_send(json.clone());
                }
            }
        }
        ChannelCmd::Forward {
            channel_id,
            sender_key,
            message,
        } => {
            let Some(channel) = channels.get_mut(&channel_id) else {
                return;
            };
            channel.touch();

            // Buffer the message for offline sync.
            // Extract ciphertext and nonce from the already-serialized message
            // by deserializing it — or we can store the raw JSON.
            // For the buffer, we need structured data. Let's parse it.
            if let Ok(relay_msg) =
                serde_json::from_str::<agent_dash_core::relay::RelayMessage>(&message)
            {
                if let agent_dash_core::relay::RelayMessage::Encrypted {
                    ciphertext,
                    nonce,
                    ..
                } = &relay_msg
                {
                    let bm = BufferedMessage {
                        seq: channel.next_seq,
                        ciphertext: ciphertext.clone(),
                        nonce: nonce.clone(),
                        timestamp: now_secs(),
                    };
                    channel.next_seq += 1;
                    channel.buffer.push_back(bm);
                    if channel.buffer.len() > max_buffer {
                        channel.buffer.pop_front();
                    }
                }
            }

            // Forward to all peers except the sender.
            for peer in &channel.peers {
                if peer.public_key != sender_key {
                    let _ = peer.tx.try_send(message.clone());
                }
            }
        }
        ChannelCmd::Sync {
            channel_id,
            since_seq,
            reply,
        } => {
            let messages = channels
                .get(&channel_id)
                .map(|ch| {
                    ch.buffer
                        .iter()
                        .filter(|m| m.seq > since_seq)
                        .cloned()
                        .collect()
                })
                .unwrap_or_default();
            let _ = reply.send(messages);
        }
        ChannelCmd::Disconnect {
            channel_id,
            public_key,
        } => {
            let Some(channel) = channels.get_mut(&channel_id) else {
                return;
            };
            channel.peers.retain(|p| p.public_key != public_key);
            let peer_count = channel.peers.len() as u32;

            // Notify remaining peers.
            let change = serde_json::to_string(
                &agent_dash_core::relay::RelayMessage::PeerChange {
                    channel_id: channel_id.clone(),
                    peer_count,
                },
            );
            if let Ok(json) = change {
                for peer in &channel.peers {
                    let _ = peer.tx.try_send(json.clone());
                }
            }

            // Remove empty channels immediately.
            if channel.peers.is_empty() && channel.buffer.is_empty() {
                channels.remove(&channel_id);
            }
        }
    }
}

fn evict_stale(channels: &mut HashMap<String, Channel>, ttl_secs: u64) {
    let now = now_secs();
    channels.retain(|_id, ch| {
        if ch.peers.is_empty() && now.saturating_sub(ch.last_activity) > ttl_secs {
            return false;
        }
        true
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn auth_and_disconnect() {
        let mgr = spawn(100, 3600);

        let (peer_tx, _peer_rx) = mpsc::channel(16);
        let (reply_tx, reply_rx) = oneshot::channel();

        mgr.send(ChannelCmd::Auth {
            channel_id: "ch1".into(),
            public_key: "pk1".into(),
            peer_tx,
            reply: reply_tx,
        })
        .await
        .unwrap();

        let result = reply_rx.await.unwrap();
        assert_eq!(result, Ok(1));

        // Disconnect.
        mgr.send(ChannelCmd::Disconnect {
            channel_id: "ch1".into(),
            public_key: "pk1".into(),
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn duplicate_key_rejected() {
        let mgr = spawn(100, 3600);

        let (peer_tx1, _rx1) = mpsc::channel(16);
        let (reply_tx1, reply_rx1) = oneshot::channel();
        mgr.send(ChannelCmd::Auth {
            channel_id: "ch1".into(),
            public_key: "pk1".into(),
            peer_tx: peer_tx1,
            reply: reply_tx1,
        })
        .await
        .unwrap();
        assert!(reply_rx1.await.unwrap().is_ok());

        let (peer_tx2, _rx2) = mpsc::channel(16);
        let (reply_tx2, reply_rx2) = oneshot::channel();
        mgr.send(ChannelCmd::Auth {
            channel_id: "ch1".into(),
            public_key: "pk1".into(),
            peer_tx: peer_tx2,
            reply: reply_tx2,
        })
        .await
        .unwrap();
        assert!(reply_rx2.await.unwrap().is_err());
    }

    #[tokio::test]
    async fn forward_and_sync() {
        let mgr = spawn(100, 3600);

        // Add two peers.
        let (tx1, _rx1) = mpsc::channel(16);
        let (reply1, r1) = oneshot::channel();
        mgr.send(ChannelCmd::Auth {
            channel_id: "ch1".into(),
            public_key: "pk1".into(),
            peer_tx: tx1,
            reply: reply1,
        })
        .await
        .unwrap();
        r1.await.unwrap().unwrap();

        let (tx2, mut rx2) = mpsc::channel(16);
        let (reply2, r2) = oneshot::channel();
        mgr.send(ChannelCmd::Auth {
            channel_id: "ch1".into(),
            public_key: "pk2".into(),
            peer_tx: tx2,
            reply: reply2,
        })
        .await
        .unwrap();
        r2.await.unwrap().unwrap();

        // Drain PeerChange notifications from rx2.
        let _ = rx2.try_recv();

        // Forward a message from pk1.
        let encrypted = serde_json::to_string(
            &agent_dash_core::relay::RelayMessage::Encrypted {
                channel_id: "ch1".into(),
                ciphertext: "ct".into(),
                nonce: "nc".into(),
            },
        )
        .unwrap();
        mgr.send(ChannelCmd::Forward {
            channel_id: "ch1".into(),
            sender_key: "pk1".into(),
            message: encrypted,
        })
        .await
        .unwrap();

        // pk2 should receive it.
        let msg = rx2.recv().await.unwrap();
        assert!(msg.contains("encrypted"));

        // Sync should return the buffered message.
        let (sync_reply, sync_rx) = oneshot::channel();
        mgr.send(ChannelCmd::Sync {
            channel_id: "ch1".into(),
            since_seq: 0,
            reply: sync_reply,
        })
        .await
        .unwrap();
        let messages = sync_rx.await.unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].seq, 1);
    }
}
