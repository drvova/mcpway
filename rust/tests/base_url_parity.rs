mod common;

use std::time::Duration;

use eventsource_stream::Eventsource;
use futures::StreamExt;

use common::{find_free_port, spawn_mcpway, stop_child, wait_for_http_status};

async fn read_endpoint_event(port: u16, sse_path: &str) -> String {
    let client = reqwest::Client::new();
    let response = client
        .get(format!("http://127.0.0.1:{port}{sse_path}"))
        .send()
        .await
        .expect("failed to connect to SSE endpoint");
    assert_eq!(response.status(), reqwest::StatusCode::OK);

    let mut stream = response.bytes_stream().eventsource();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match stream.next().await {
                Some(Ok(event)) if event.event == "endpoint" => break event.data,
                Some(Ok(_)) => continue,
                Some(Err(err)) => panic!("SSE stream error: {err}"),
                None => panic!("SSE stream ended before endpoint event"),
            }
        }
    })
    .await
    .expect("timed out waiting for endpoint event")
}

#[tokio::test]
async fn endpoint_event_uses_base_url_when_configured() {
    let port = find_free_port();
    let port_str = port.to_string();
    let sse_path = "/events";
    let message_path = "/rpc";
    let base_url = format!("http://127.0.0.1:{port}");

    let mut child = spawn_mcpway(
        &[
            "--stdio",
            "cat",
            "--port",
            &port_str,
            "--baseUrl",
            &base_url,
            "--ssePath",
            sse_path,
            "--messagePath",
            message_path,
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

    let endpoint = read_endpoint_event(port, sse_path).await;
    assert!(
        endpoint.starts_with(&format!("{base_url}{message_path}?sessionId=")),
        "endpoint should include configured baseUrl, got: {endpoint}"
    );

    stop_child(&mut child).await;
}

#[tokio::test]
async fn endpoint_event_is_relative_without_base_url() {
    let port = find_free_port();
    let port_str = port.to_string();
    let sse_path = "/events";
    let message_path = "/rpc";

    let mut child = spawn_mcpway(
        &[
            "--stdio",
            "cat",
            "--port",
            &port_str,
            "--ssePath",
            sse_path,
            "--messagePath",
            message_path,
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

    let endpoint = read_endpoint_event(port, sse_path).await;
    assert!(
        endpoint.starts_with(&format!("{message_path}?sessionId=")),
        "endpoint should be relative without baseUrl, got: {endpoint}"
    );

    stop_child(&mut child).await;
}
