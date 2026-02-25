mod common;

use std::time::Duration;

use futures::future::join_all;
use futures::{SinkExt, StreamExt};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

use common::{find_free_port, initialize_request, spawn_mcpway, stop_child, wait_for_http_status};

#[tokio::test]
async fn stdio_to_ws_handles_many_parallel_clients() {
    let port = find_free_port();
    let port_str = port.to_string();

    let mut child = spawn_mcpway(
        &[
            "--stdio",
            "cat",
            "--outputTransport",
            "ws",
            "--port",
            &port_str,
            "--messagePath",
            "/message",
            "--healthEndpoint",
            "/healthz",
            "--logLevel",
            "none",
        ],
        false,
        false,
    )
    .await;

    wait_for_http_status(
        &format!("http://127.0.0.1:{port}/healthz"),
        reqwest::StatusCode::OK,
        Duration::from_secs(10),
    )
    .await;

    let ws_url = format!("ws://127.0.0.1:{port}/message");
    let client_count = 32usize;
    let tasks = (0..client_count).map(|idx| {
        let ws_url = ws_url.clone();
        async move {
            let (mut socket, _) = connect_async(&ws_url)
                .await
                .expect("failed to connect websocket client");

            let request_id = format!("ws-concurrency-{idx}");
            let request = initialize_request(&request_id);
            socket
                .send(Message::Text(request.to_string().into()))
                .await
                .expect("failed to send websocket request");

            let response = tokio::time::timeout(Duration::from_secs(10), socket.next())
                .await
                .expect("timed out waiting for websocket response")
                .expect("websocket closed before response")
                .expect("websocket returned error");
            let text = response.into_text().expect("websocket frame was not text");
            let payload: serde_json::Value =
                serde_json::from_str(&text).expect("websocket payload was not valid JSON");
            assert_eq!(payload.get("id"), Some(&serde_json::json!(request_id)));
            assert_eq!(
                payload.get("method"),
                Some(&serde_json::json!("initialize"))
            );
        }
    });

    tokio::time::timeout(Duration::from_secs(45), join_all(tasks))
        .await
        .expect("timed out waiting for websocket concurrency run");

    stop_child(&mut child).await;
}
