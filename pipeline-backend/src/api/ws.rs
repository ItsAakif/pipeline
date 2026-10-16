use actix_ws::{Message, Session, MessageStream};
use futures::StreamExt;
use serde::{Deserialize, Serialize};

/// WebSocket message from client
#[derive(Debug, Deserialize)]
struct WsMessage {
    msg_type: String,
    /// Part of the client wire protocol; accepted (and validated by serde) but
    /// not read by the current handlers, hence the targeted allow.
    #[allow(dead_code)]
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<serde_json::Value>,
}

/// WebSocket response to client
#[derive(Debug, Serialize)]
struct WsResponse {
    msg_type: String,
    /// Serialized into the outgoing JSON payload; not read by our own code,
    /// hence the targeted allow rather than removing the field.
    #[allow(dead_code)]
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<serde_json::Value>,
    timestamp: u64,
}

pub async fn handle_ws(
    mut session: Session,
    mut msg_stream: MessageStream,
) -> Result<(), Box<dyn std::error::Error>> {
    while let Some(result) = msg_stream.next().await {
        match result {
            Ok(msg) => {
                match msg {
                    Message::Ping(bytes) => {
                        session.pong(&bytes).await?;
                    }
                    Message::Pong(_) => {
                        log::debug!("Received pong from client");
                    }
                    Message::Text(text) => {
                        log::info!("Received WebSocket message: {}", text);

                        // Parse and handle client message
                        if let Ok(ws_msg) = serde_json::from_str::<WsMessage>(&text) {
                            match ws_msg.msg_type.as_str() {
                                "ping" => {
                                    let response = WsResponse {
                                        msg_type: "pong".to_string(),
                                        data: None,
                                        timestamp: std::time::SystemTime::now()
                                            .duration_since(std::time::UNIX_EPOCH)
                                            .unwrap()
                                            .as_secs(),
                                    };

                                    if let Ok(json) = serde_json::to_string(&response) {
                                        session.text(json).await?;
                                    }
                                }
                                "subscribe_execution" => {
                                    log::info!("Client subscribed to execution updates");
                                    let response = WsResponse {
                                        msg_type: "subscription_confirmed".to_string(),
                                        data: Some(serde_json::json!({"topic": "execution"})),
                                        timestamp: std::time::SystemTime::now()
                                            .duration_since(std::time::UNIX_EPOCH)
                                            .unwrap()
                                            .as_secs(),
                                    };

                                    if let Ok(json) = serde_json::to_string(&response) {
                                        session.text(json).await?;
                                    }
                                }
                                _ => {
                                    log::warn!("Unknown WebSocket message type: {}", ws_msg.msg_type);
                                    let response = WsResponse {
                                        msg_type: "error".to_string(),
                                        data: Some(serde_json::json!({"message": "Unknown message type"})),
                                        timestamp: std::time::SystemTime::now()
                                            .duration_since(std::time::UNIX_EPOCH)
                                            .unwrap()
                                            .as_secs(),
                                    };

                                    if let Ok(json) = serde_json::to_string(&response) {
                                        session.text(json).await?;
                                    }
                                }
                            }
                        } else {
                            log::warn!("Failed to parse WebSocket message: {}", text);
                        }
                    }
                    Message::Close(reason) => {
                        log::info!("WebSocket connection closed: {:?}", reason);
                        return Ok(());
                    }
                    Message::Continuation(_) => {
                        // Handle continuation frames
                    }
                    Message::Nop => {
                        // No-op
                    }
                    _ => {}
                }
            }
            Err(e) => {
                log::error!("WebSocket error: {:?}", e);
                return Err(Box::new(e) as Box<dyn std::error::Error>);
            }
        }
    }

    Ok(())
}
