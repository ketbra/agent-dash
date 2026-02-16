use serde::{Deserialize, Serialize};

/// Messages exchanged between clients and the relay server over WebSocket.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum RelayMessage {
    /// Client authenticates to join a channel.
    #[serde(rename = "auth")]
    Auth {
        channel_id: String,
        public_key: String,
    },

    /// Relay -> client: auth accepted.
    #[serde(rename = "auth_ok")]
    AuthOk { peer_count: u32 },

    /// Relay -> client: auth rejected.
    #[serde(rename = "auth_error")]
    AuthError { message: String },

    /// Encrypted application payload (daemon events / phone commands).
    #[serde(rename = "encrypted")]
    Encrypted {
        channel_id: String,
        ciphertext: String,
        nonce: String,
    },

    /// Relay -> client: peer joined or left.
    #[serde(rename = "peer_change")]
    PeerChange { channel_id: String, peer_count: u32 },

    /// Client -> relay: catch up on missed messages.
    #[serde(rename = "sync")]
    Sync { channel_id: String, since_seq: u64 },

    /// Relay -> client: buffered messages.
    #[serde(rename = "sync_response")]
    SyncResponse {
        channel_id: String,
        messages: Vec<BufferedMessage>,
    },
}

/// A message stored in the relay's ring buffer for offline sync.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BufferedMessage {
    pub seq: u64,
    pub ciphertext: String,
    pub nonce: String,
    pub timestamp: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialize_auth() {
        let msg = RelayMessage::Auth {
            channel_id: "abc123".into(),
            public_key: "cGsK".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"type\":\"auth\""));
        assert!(json.contains("\"channel_id\":\"abc123\""));
        assert!(json.contains("\"public_key\":\"cGsK\""));
    }

    #[test]
    fn round_trip_auth() {
        let msg = RelayMessage::Auth {
            channel_id: "ch1".into(),
            public_key: "pk1".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: RelayMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            RelayMessage::Auth { channel_id, public_key } => {
                assert_eq!(channel_id, "ch1");
                assert_eq!(public_key, "pk1");
            }
            _ => panic!("expected Auth"),
        }
    }

    #[test]
    fn round_trip_auth_ok() {
        let msg = RelayMessage::AuthOk { peer_count: 2 };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"type\":\"auth_ok\""));
        let decoded: RelayMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            RelayMessage::AuthOk { peer_count } => assert_eq!(peer_count, 2),
            _ => panic!("expected AuthOk"),
        }
    }

    #[test]
    fn round_trip_auth_error() {
        let msg = RelayMessage::AuthError {
            message: "bad channel".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: RelayMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            RelayMessage::AuthError { message } => assert_eq!(message, "bad channel"),
            _ => panic!("expected AuthError"),
        }
    }

    #[test]
    fn round_trip_encrypted() {
        let msg = RelayMessage::Encrypted {
            channel_id: "ch1".into(),
            ciphertext: "Y2lwaGVy".into(),
            nonce: "bm9uY2U=".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"type\":\"encrypted\""));
        let decoded: RelayMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            RelayMessage::Encrypted { channel_id, ciphertext, nonce } => {
                assert_eq!(channel_id, "ch1");
                assert_eq!(ciphertext, "Y2lwaGVy");
                assert_eq!(nonce, "bm9uY2U=");
            }
            _ => panic!("expected Encrypted"),
        }
    }

    #[test]
    fn round_trip_peer_change() {
        let msg = RelayMessage::PeerChange {
            channel_id: "ch1".into(),
            peer_count: 3,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: RelayMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            RelayMessage::PeerChange { channel_id, peer_count } => {
                assert_eq!(channel_id, "ch1");
                assert_eq!(peer_count, 3);
            }
            _ => panic!("expected PeerChange"),
        }
    }

    #[test]
    fn round_trip_sync() {
        let msg = RelayMessage::Sync {
            channel_id: "ch1".into(),
            since_seq: 42,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"type\":\"sync\""));
        let decoded: RelayMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            RelayMessage::Sync { channel_id, since_seq } => {
                assert_eq!(channel_id, "ch1");
                assert_eq!(since_seq, 42);
            }
            _ => panic!("expected Sync"),
        }
    }

    #[test]
    fn round_trip_sync_response() {
        let msg = RelayMessage::SyncResponse {
            channel_id: "ch1".into(),
            messages: vec![
                BufferedMessage {
                    seq: 1,
                    ciphertext: "ct1".into(),
                    nonce: "n1".into(),
                    timestamp: 1000,
                },
                BufferedMessage {
                    seq: 2,
                    ciphertext: "ct2".into(),
                    nonce: "n2".into(),
                    timestamp: 2000,
                },
            ],
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: RelayMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            RelayMessage::SyncResponse { channel_id, messages } => {
                assert_eq!(channel_id, "ch1");
                assert_eq!(messages.len(), 2);
                assert_eq!(messages[0].seq, 1);
                assert_eq!(messages[1].timestamp, 2000);
            }
            _ => panic!("expected SyncResponse"),
        }
    }

    #[test]
    fn deserialize_from_json_string() {
        let json = r#"{"type":"auth","channel_id":"abc","public_key":"pk"}"#;
        let msg: RelayMessage = serde_json::from_str(json).unwrap();
        assert!(matches!(msg, RelayMessage::Auth { .. }));
    }

    #[test]
    fn buffered_message_round_trip() {
        let bm = BufferedMessage {
            seq: 99,
            ciphertext: "data".into(),
            nonce: "nonce".into(),
            timestamp: 12345,
        };
        let json = serde_json::to_string(&bm).unwrap();
        let decoded: BufferedMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.seq, 99);
        assert_eq!(decoded.timestamp, 12345);
    }
}
