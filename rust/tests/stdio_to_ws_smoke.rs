mod common;

use std::time::Duration;

use futures::{SinkExt, StreamExt};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

use common::{
    find_free_port, initialize_request, spawn_mcpway, stop_child, wait_for_http_status,
};

#[tokio::test]
async fn stdio_to_ws_roundtrip_smoke() {
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

    let (mut socket, _response) = connect_async(format!("ws://127.0.0.1:{port}/message"))
        .await
        .expect("failed to connect to websocket endpoint");

    let initialize = initialize_request("ws-init");
    socket
        .send(Message::Text(initialize.to_string()))
        .await
        .expect("failed to send initialize message over websocket");

    let response_message = tokio::time::timeout(Duration::from_secs(5), socket.next())
        .await
        .expect("timed out waiting for websocket response")
        .expect("websocket closed before response")
        .expect("websocket returned error");

    let text = response_message
        .into_text()
        .expect("websocket response was not text");
    let payload: serde_json::Value =
        serde_json::from_str(&text).expect("websocket response was not valid JSON");

    assert_eq!(payload.get("id"), Some(&serde_json::json!("ws-init")));
    assert_eq!(
        payload.get("method"),
        Some(&serde_json::json!("initialize"))
    );

    stop_child(&mut child).await;
}
