mod common;

use std::time::Duration;

use common::{find_free_port, spawn_mcpway, stop_child, wait_for_http_status};

#[tokio::test]
async fn stateful_streamable_session_expires_after_timeout() {
    let port = find_free_port();
    let port_str = port.to_string();

    let mut child = spawn_mcpway(
        &[
            "--stdio",
            "cat",
            "--outputTransport",
            "streamable-http",
            "--stateful",
            "--sessionTimeout",
            "200",
            "--port",
            &port_str,
            "--streamableHttpPath",
            "/mcp",
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

    let client = reqwest::Client::new();
    let initialize = serde_json::json!({
        "jsonrpc": "2.0",
        "id": "timeout-init",
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "timeout-test", "version": "1.0.0" }
        }
    });

    let initialize_response = client
        .post(format!("http://127.0.0.1:{port}/mcp"))
        .json(&initialize)
        .send()
        .await
        .expect("failed to initialize stateful streamable session");
    assert_eq!(initialize_response.status(), reqwest::StatusCode::OK);

    let session_id = initialize_response
        .headers()
        .get("Mcp-Session-Id")
        .or_else(|| initialize_response.headers().get("mcp-session-id"))
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
        .expect("missing Mcp-Session-Id header on initialize response");

    let _payload: serde_json::Value = initialize_response
        .json()
        .await
        .expect("initialize response was not JSON");

    tokio::time::sleep(Duration::from_millis(1200)).await;

    let expired = client
        .get(format!("http://127.0.0.1:{port}/mcp"))
        .header("Accept", "text/event-stream")
        .header("Mcp-Session-Id", &session_id)
        .send()
        .await
        .expect("failed to call stateful streamable endpoint after timeout");

    assert_eq!(expired.status(), reqwest::StatusCode::BAD_REQUEST);

    stop_child(&mut child).await;
}
