mod common;

use std::time::Duration;

use futures::{SinkExt, StreamExt};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::oneshot;
use tokio_tungstenite::tungstenite::Message;

use common::{spawn_mcpway, stop_child};

#[tokio::test]
async fn connect_ws_to_stdio_roundtrip_text_frame() {
    run_ws_roundtrip(false, "ws-text-id").await;
}

#[tokio::test]
async fn connect_ws_to_stdio_roundtrip_binary_frame() {
    run_ws_roundtrip(true, "ws-binary-id").await;
}

async fn run_ws_roundtrip(binary_response: bool, request_id: &str) {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("failed to bind websocket listener");
    let port = listener
        .local_addr()
        .expect("failed to read websocket listener addr")
        .port();

    let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();
    let server = tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = &mut shutdown_rx => {
                    break;
                }
                incoming = listener.accept() => {
                    let Ok((stream, _addr)) = incoming else {
                        continue;
                    };
                    tokio::spawn(async move {
                        let mut ws = tokio_tungstenite::accept_async(stream)
                            .await
                            .expect("websocket accept failed");

                        while let Some(frame) = ws.next().await {
                            let Ok(frame) = frame else {
                                break;
                            };

                            let parsed = match frame {
                                Message::Text(text) => serde_json::from_str::<serde_json::Value>(&text)
                                    .expect("text frame was not valid JSON"),
                                Message::Binary(bytes) => serde_json::from_slice::<serde_json::Value>(&bytes)
                                    .expect("binary frame was not valid JSON"),
                                Message::Close(_) => break,
                                Message::Ping(_) | Message::Pong(_) => continue,
                                _ => continue,
                            };

                            let id = parsed
                                .get("id")
                                .cloned()
                                .unwrap_or(serde_json::Value::Null);
                            let response = serde_json::json!({
                                "jsonrpc": "2.0",
                                "id": id,
                                "result": {
                                    "ok": true
                                }
                            })
                            .to_string();

                            let outbound = if binary_response {
                                Message::Binary(response.into_bytes().into())
                            } else {
                                Message::Text(response.into())
                            };
                            ws.send(outbound)
                                .await
                                .expect("failed to send websocket response");
                        }
                    });
                }
            }
        }
    });

    let mut child = spawn_mcpway(
        &[
            "connect",
            &format!("ws://127.0.0.1:{port}/ws"),
            "--protocol",
            "ws",
            "--log-level",
            "none",
        ],
        true,
        true,
    )
    .await;

    let stdin = child.stdin.as_mut().expect("stdin was not piped");
    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": request_id,
        "method": "tools/list",
        "params": {}
    });
    stdin
        .write_all(format!("{request}\n").as_bytes())
        .await
        .expect("failed to write request to stdin");
    stdin.flush().await.expect("failed to flush stdin");

    let stdout = child.stdout.take().expect("stdout was not piped");
    let mut lines = BufReader::new(stdout).lines();
    let line = tokio::time::timeout(Duration::from_secs(10), lines.next_line())
        .await
        .expect("timed out waiting for stdout response")
        .expect("failed reading stdout line")
        .expect("stdout closed before response line");

    let payload: serde_json::Value =
        serde_json::from_str(&line).expect("stdout line was not valid JSON");
    assert_eq!(payload.get("id"), Some(&serde_json::json!(request_id)));
    assert_eq!(
        payload.get("result").and_then(|result| result.get("ok")),
        Some(&serde_json::json!(true))
    );

    stop_child(&mut child).await;
    let _ = shutdown_tx.send(());
    let _ = server.await;
}
