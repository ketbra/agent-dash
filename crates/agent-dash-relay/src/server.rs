use std::net::SocketAddr;
use std::sync::Arc;

use agent_dash_core::relay::RelayMessage;
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;

use crate::channel::ChannelCmd;

/// Handle a single WebSocket connection.
pub async fn handle_connection(
    stream: TcpStream,
    addr: SocketAddr,
    channel_mgr: mpsc::Sender<ChannelCmd>,
    required_token: Option<Arc<str>>,
) {
    let ws_stream = match tokio_tungstenite::accept_async(stream).await {
        Ok(ws) => ws,
        Err(e) => {
            eprintln!("[{addr}] WebSocket handshake failed: {e}");
            return;
        }
    };

    let (mut ws_tx, mut ws_rx) = ws_stream.split();

    // Wait for Auth message first.
    let (channel_id, public_key) = loop {
        let Some(msg) = ws_rx.next().await else {
            return;
        };
        let msg = match msg {
            Ok(Message::Text(t)) => t,
            Ok(Message::Close(_)) | Err(_) => return,
            Ok(Message::Ping(data)) => {
                let _ = ws_tx.send(Message::Pong(data)).await;
                continue;
            }
            _ => continue,
        };

        match serde_json::from_str::<RelayMessage>(&msg) {
            Ok(RelayMessage::Auth {
                channel_id,
                public_key,
                server_token,
            }) => {
                // Validate server access token if the relay requires one.
                if let Some(ref expected) = required_token {
                    match server_token {
                        Some(ref provided) if provided.as_str() == &**expected => {
                            // Token matches — allow.
                        }
                        _ => {
                            let err = RelayMessage::AuthError {
                                message: "invalid or missing server token".into(),
                            };
                            let _ = ws_tx
                                .send(Message::Text(
                                    serde_json::to_string(&err).unwrap().into(),
                                ))
                                .await;
                            return;
                        }
                    }
                }
                break (channel_id, public_key);
            }
            Ok(_) => {
                let err = RelayMessage::AuthError {
                    message: "must authenticate first".into(),
                };
                let _ = ws_tx
                    .send(Message::Text(serde_json::to_string(&err).unwrap().into()))
                    .await;
                return;
            }
            Err(e) => {
                let err = RelayMessage::AuthError {
                    message: format!("invalid message: {e}"),
                };
                let _ = ws_tx
                    .send(Message::Text(serde_json::to_string(&err).unwrap().into()))
                    .await;
                return;
            }
        }
    };

    // Register with channel manager.
    let (peer_tx, mut peer_rx) = mpsc::channel::<String>(64);
    let (reply_tx, reply_rx) = oneshot::channel();

    if channel_mgr
        .send(ChannelCmd::Auth {
            channel_id: channel_id.clone(),
            public_key: public_key.clone(),
            peer_tx,
            reply: reply_tx,
        })
        .await
        .is_err()
    {
        return;
    }

    match reply_rx.await {
        Ok(Ok(peer_count)) => {
            let ok = RelayMessage::AuthOk { peer_count };
            let _ = ws_tx
                .send(Message::Text(serde_json::to_string(&ok).unwrap().into()))
                .await;
        }
        Ok(Err(e)) => {
            let err = RelayMessage::AuthError { message: e };
            let _ = ws_tx
                .send(Message::Text(serde_json::to_string(&err).unwrap().into()))
                .await;
            return;
        }
        Err(_) => return,
    }

    println!("[{addr}] authenticated on channel {}", &channel_id[..8.min(channel_id.len())]);

    // Main forwarding loop.
    loop {
        tokio::select! {
            // Messages from other peers (via channel manager).
            Some(outgoing) = peer_rx.recv() => {
                if ws_tx.send(Message::Text(outgoing.into())).await.is_err() {
                    break;
                }
            }
            // Messages from this WebSocket client.
            msg = ws_rx.next() => {
                let Some(msg) = msg else { break };
                let text = match msg {
                    Ok(Message::Text(t)) => t,
                    Ok(Message::Close(_)) | Err(_) => break,
                    Ok(Message::Ping(data)) => {
                        let _ = ws_tx.send(Message::Pong(data)).await;
                        continue;
                    }
                    _ => continue,
                };

                match serde_json::from_str::<RelayMessage>(&text) {
                    Ok(RelayMessage::Encrypted { .. }) => {
                        let _ = channel_mgr
                            .send(ChannelCmd::Forward {
                                channel_id: channel_id.clone(),
                                sender_key: public_key.clone(),
                                message: text.to_string(),
                            })
                            .await;
                    }
                    Ok(RelayMessage::Sync {
                        channel_id: ref cid,
                        since_seq,
                    }) => {
                        let (reply_tx, reply_rx) = oneshot::channel();
                        let _ = channel_mgr
                            .send(ChannelCmd::Sync {
                                channel_id: cid.clone(),
                                since_seq,
                                reply: reply_tx,
                            })
                            .await;
                        if let Ok(messages) = reply_rx.await {
                            let resp = RelayMessage::SyncResponse {
                                channel_id: cid.clone(),
                                messages,
                            };
                            let _ = ws_tx
                                .send(Message::Text(serde_json::to_string(&resp).unwrap().into()))
                                .await;
                        }
                    }
                    _ => {
                        // Ignore unexpected messages from authenticated clients.
                    }
                }
            }
        }
    }

    // Disconnect cleanup.
    let _ = channel_mgr
        .send(ChannelCmd::Disconnect {
            channel_id,
            public_key,
        })
        .await;

    println!("[{addr}] disconnected");
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;
    use tokio_tungstenite::connect_async;

    /// Helper: start a relay on an ephemeral port with an optional token,
    /// returning the WebSocket URL.
    async fn start_relay(token: Option<&str>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let channel_mgr = crate::channel::spawn(100, 3600);
        let required_token: Option<Arc<str>> = token.map(|t| Arc::from(t));

        tokio::spawn(async move {
            loop {
                let (stream, addr) = listener.accept().await.unwrap();
                let mgr = channel_mgr.clone();
                let tok = required_token.clone();
                tokio::spawn(handle_connection(stream, addr, mgr, tok));
            }
        });

        format!("ws://127.0.0.1:{port}")
    }

    #[tokio::test]
    async fn auth_without_token_when_not_required() {
        let url = start_relay(None).await;
        let (ws, _) = connect_async(&url).await.unwrap();
        let (mut tx, mut rx) = ws.split();

        let auth = RelayMessage::Auth {
            channel_id: "ch1".into(),
            public_key: "pk1".into(),
            server_token: None,
        };
        tx.send(Message::Text(serde_json::to_string(&auth).unwrap().into()))
            .await
            .unwrap();

        let resp = rx.next().await.unwrap().unwrap();
        let text = match resp {
            Message::Text(t) => t,
            other => panic!("expected text, got {other:?}"),
        };
        let msg: RelayMessage = serde_json::from_str(&text).unwrap();
        assert!(matches!(msg, RelayMessage::AuthOk { .. }));
    }

    #[tokio::test]
    async fn auth_with_correct_token() {
        let url = start_relay(Some("my-secret")).await;
        let (ws, _) = connect_async(&url).await.unwrap();
        let (mut tx, mut rx) = ws.split();

        let auth = RelayMessage::Auth {
            channel_id: "ch1".into(),
            public_key: "pk1".into(),
            server_token: Some("my-secret".into()),
        };
        tx.send(Message::Text(serde_json::to_string(&auth).unwrap().into()))
            .await
            .unwrap();

        let resp = rx.next().await.unwrap().unwrap();
        let msg: RelayMessage = serde_json::from_str(&*match resp {
            Message::Text(t) => t,
            other => panic!("expected text, got {other:?}"),
        })
        .unwrap();
        assert!(matches!(msg, RelayMessage::AuthOk { .. }));
    }

    #[tokio::test]
    async fn auth_rejected_with_wrong_token() {
        let url = start_relay(Some("my-secret")).await;
        let (ws, _) = connect_async(&url).await.unwrap();
        let (mut tx, mut rx) = ws.split();

        let auth = RelayMessage::Auth {
            channel_id: "ch1".into(),
            public_key: "pk1".into(),
            server_token: Some("wrong-token".into()),
        };
        tx.send(Message::Text(serde_json::to_string(&auth).unwrap().into()))
            .await
            .unwrap();

        let resp = rx.next().await.unwrap().unwrap();
        let msg: RelayMessage = serde_json::from_str(&*match resp {
            Message::Text(t) => t,
            other => panic!("expected text, got {other:?}"),
        })
        .unwrap();
        match msg {
            RelayMessage::AuthError { message } => {
                assert!(message.contains("invalid or missing server token"));
            }
            other => panic!("expected AuthError, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn auth_rejected_with_missing_token() {
        let url = start_relay(Some("my-secret")).await;
        let (ws, _) = connect_async(&url).await.unwrap();
        let (mut tx, mut rx) = ws.split();

        let auth = RelayMessage::Auth {
            channel_id: "ch1".into(),
            public_key: "pk1".into(),
            server_token: None,
        };
        tx.send(Message::Text(serde_json::to_string(&auth).unwrap().into()))
            .await
            .unwrap();

        let resp = rx.next().await.unwrap().unwrap();
        let msg: RelayMessage = serde_json::from_str(&*match resp {
            Message::Text(t) => t,
            other => panic!("expected text, got {other:?}"),
        })
        .unwrap();
        match msg {
            RelayMessage::AuthError { message } => {
                assert!(message.contains("invalid or missing server token"));
            }
            other => panic!("expected AuthError, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn token_not_required_allows_any_client() {
        let url = start_relay(None).await;
        let (ws, _) = connect_async(&url).await.unwrap();
        let (mut tx, mut rx) = ws.split();

        // Client sends a token even though relay doesn't require one — should still work.
        let auth = RelayMessage::Auth {
            channel_id: "ch1".into(),
            public_key: "pk1".into(),
            server_token: Some("extra-token".into()),
        };
        tx.send(Message::Text(serde_json::to_string(&auth).unwrap().into()))
            .await
            .unwrap();

        let resp = rx.next().await.unwrap().unwrap();
        let msg: RelayMessage = serde_json::from_str(&*match resp {
            Message::Text(t) => t,
            other => panic!("expected text, got {other:?}"),
        })
        .unwrap();
        assert!(matches!(msg, RelayMessage::AuthOk { .. }));
    }
}
