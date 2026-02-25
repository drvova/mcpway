mod common;

use std::time::Duration;

use futures::future::join_all;

use common::{find_free_port, initialize_request, spawn_mcpway, stop_child, wait_for_http_status};

#[tokio::test]
async fn stdio_to_streamable_http_stateful_handles_many_inflight_requests() {
    let port = find_free_port();
    let port_str = port.to_string();

    let mut child = spawn_mcpway(
        &[
            "--stdio",
            "cat",
            "--outputTransport",
            "streamable-http",
            "--stateful",
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
    let url = format!("http://127.0.0.1:{port}/mcp");

    let initialize = initialize_request("streamable-concurrency-init");
    let init_response = client
        .post(&url)
        .json(&initialize)
        .send()
        .await
        .expect("failed to initialize stateful streamable-http session");
    assert_eq!(init_response.status(), reqwest::StatusCode::OK);

    let session_id = init_response
        .headers()
        .get("Mcp-Session-Id")
        .or_else(|| init_response.headers().get("mcp-session-id"))
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
        .expect("missing Mcp-Session-Id on initialize response");

    let request_count = 120usize;
    let tasks = (0..request_count).map(|idx| {
        let client = client.clone();
        let url = url.clone();
        let session_id = session_id.clone();
        async move {
            let request = serde_json::json!({
                "jsonrpc": "2.0",
                "id": format!("streamable-concurrency-{idx}"),
                "method": "tools/list",
                "params": {}
            });

            let response = client
                .post(&url)
                .header("Mcp-Session-Id", &session_id)
                .json(&request)
                .send()
                .await
                .expect("concurrent request failed");
            assert_eq!(response.status(), reqwest::StatusCode::OK);

            let body: serde_json::Value = response
                .json()
                .await
                .expect("response body was not valid JSON");
            assert_eq!(body.get("id"), request.get("id"));
            assert_eq!(body.get("method"), request.get("method"));
        }
    });

    tokio::time::timeout(Duration::from_secs(45), join_all(tasks))
        .await
        .expect("timed out waiting for concurrent stateful streamable-http requests");

    stop_child(&mut child).await;
}
