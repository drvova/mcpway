use std::time::Duration;

use futures::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;
use tokio_util::codec::{FramedRead, LinesCodec};

use crate::runtime::store::RuntimeArgsStore;
use crate::runtime::{RuntimeApplyResult, RuntimeScope, RuntimeUpdateRequest};
use crate::support::signals::install_signal_handlers;
use crate::transport::pool::{global_pool, transport_fingerprint};

const WS_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

pub async fn run(
    endpoint: String,
    protocol_version: String,
    runtime: RuntimeArgsStore,
    mut updates: mpsc::Receiver<RuntimeUpdateRequest>,
) -> Result<(), String> {
    tracing::info!("  - ws: {endpoint}");
    tracing::info!("Connecting to WebSocket endpoint...");

    install_signal_handlers(None);

    let initial_runtime = runtime.get_effective(None).await;
    let warm_key =
        transport_fingerprint("ws", &endpoint, &initial_runtime.headers, &protocol_version);
    let request = build_ws_request(&endpoint, &initial_runtime.headers)?;
    let (stream, _) = tokio::time::timeout(
        WS_CONNECT_TIMEOUT,
        tokio_tungstenite::connect_async(request),
    )
    .await
    .map_err(|_| {
        format!(
            "Timed out connecting to WebSocket endpoint after {}ms",
            WS_CONNECT_TIMEOUT.as_millis()
        )
    })?
    .map_err(|err| format!("WebSocket connection failed: {err}"))?;
    global_pool().mark_success(&warm_key, "ws").await;

    let runtime_store = runtime.clone();
    tokio::spawn(async move {
        while let Some(req) = updates.recv().await {
            let result = match req.update.scope {
                RuntimeScope::Global => {
                    let update_result = runtime_store.update_global(req.update.update).await;
                    if update_result.restart_needed || update_result.headers_changed {
                        RuntimeApplyResult::ok(
                            "Updated runtime args/headers; reconnect required for WebSocket endpoint",
                            true,
                        )
                    } else {
                        RuntimeApplyResult::ok("No runtime changes applied", false)
                    }
                }
                RuntimeScope::Session(_) => RuntimeApplyResult::error(
                    "Per-session runtime overrides are not supported for WebSocket→stdio",
                ),
            };
            let _ = req.respond_to.send(result);
        }
    });

    let (mut ws_writer, mut ws_reader) = stream.split();
    let mut stdin_lines = FramedRead::new(tokio::io::stdin(), LinesCodec::new());

    loop {
        tokio::select! {
            line = stdin_lines.next() => {
                let Some(line) = line else {
                    break;
                };
                let line = line.map_err(|err| err.to_string())?;
                if line.trim().is_empty() {
                    continue;
                }

                let Ok(message_json) = serde_json::from_str::<serde_json::Value>(&line) else {
                    tracing::error!("Invalid JSON from stdin: {line}");
                    continue;
                };

                if !is_request(&message_json) {
                    println!("{}", message_json);
                    continue;
                }

                let outbound = Message::Text(message_json.to_string());
                ws_writer
                    .send(outbound)
                    .await
                    .map_err(|err| format!("Failed to write WebSocket message: {err}"))?;
            }
            frame = ws_reader.next() => {
                let Some(frame) = frame else {
                    return Err("WebSocket connection closed".to_string());
                };
                let frame = frame.map_err(|err| format!("WebSocket stream error: {err}"))?;
                handle_incoming_frame(frame)?;
            }
        }
    }

    Ok(())
}

fn build_ws_request(
    endpoint: &str,
    headers: &std::collections::HashMap<String, String>,
) -> Result<tokio_tungstenite::tungstenite::http::Request<()>, String> {
    let mut request = endpoint
        .into_client_request()
        .map_err(|err| format!("Invalid WebSocket endpoint {endpoint}: {err}"))?;
    for (key, value) in headers {
        let header_name =
            tokio_tungstenite::tungstenite::http::header::HeaderName::from_bytes(key.as_bytes())
                .map_err(|err| format!("Invalid header name '{key}': {err}"))?;
        let header_value =
            tokio_tungstenite::tungstenite::http::header::HeaderValue::from_str(value)
                .map_err(|err| format!("Invalid header value for '{key}': {err}"))?;
        request.headers_mut().insert(header_name, header_value);
    }
    Ok(request)
}

fn handle_incoming_frame(frame: Message) -> Result<(), String> {
    match frame {
        Message::Text(text) => {
            let payload: serde_json::Value = serde_json::from_str(&text)
                .map_err(|err| format!("WebSocket text frame was not valid JSON: {err}"))?;
            println!("{}", payload);
        }
        Message::Binary(bytes) => {
            let payload: serde_json::Value = serde_json::from_slice(&bytes)
                .map_err(|err| format!("WebSocket binary frame was not valid JSON: {err}"))?;
            println!("{}", payload);
        }
        Message::Close(close_frame) => {
            if let Some(frame) = close_frame {
                return Err(format!(
                    "WebSocket closed by peer (code={}, reason={})",
                    frame.code, frame.reason
                ));
            }
            return Err("WebSocket closed by peer".to_string());
        }
        Message::Ping(_) | Message::Pong(_) => {}
        _ => {}
    }
    Ok(())
}

fn is_request(message: &serde_json::Value) -> bool {
    message.get("method").is_some() && message.get("id").is_some()
}
