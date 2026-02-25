mod common;

use std::collections::HashSet;
use std::time::Duration;

use eventsource_stream::Eventsource;
use futures::future::join_all;
use futures::StreamExt;

use common::{find_free_port, spawn_mcpway, stop_child, wait_for_http_status};

#[tokio::test]
async fn stdio_to_sse_handles_high_event_volume_without_stalling() {
    let port = find_free_port();
    let port_str = port.to_string();

    let mut child = spawn_mcpway(
        &[
            "--stdio",
            "cat",
            "--port",
            &port_str,
            "--ssePath",
            "/sse",
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

    let client = reqwest::Client::new();
    let sse_response = client
        .get(format!("http://127.0.0.1:{port}/sse"))
        .send()
        .await
        .expect("failed to connect to SSE endpoint");
    assert_eq!(sse_response.status(), reqwest::StatusCode::OK);
    let mut stream = sse_response.bytes_stream().eventsource();

    let endpoint = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match stream.next().await {
                Some(Ok(event)) if event.event == "endpoint" => break event.data,
                Some(Ok(_)) => continue,
                Some(Err(err)) => panic!("SSE stream error while waiting for endpoint: {err}"),
                None => panic!("SSE stream ended before endpoint event"),
            }
        }
    })
    .await
    .expect("timed out waiting for endpoint event");

    let endpoint_url = if endpoint.starts_with("http://") || endpoint.starts_with("https://") {
        endpoint
    } else {
        format!("http://127.0.0.1:{port}{endpoint}")
    };

    let request_count = 320usize;
    let sender = async {
        let tasks = (0..request_count).map(|idx| {
            let client = client.clone();
            let endpoint_url = endpoint_url.clone();
            async move {
                let payload = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": format!("sse-concurrency-{idx}"),
                    "method": "tools/list",
                    "params": {}
                });
                let response = client
                    .post(&endpoint_url)
                    .json(&payload)
                    .send()
                    .await
                    .expect("failed to post payload to stdio->sse message endpoint");
                assert_eq!(response.status(), reqwest::StatusCode::OK);
            }
        });
        join_all(tasks).await;
    };

    let receiver = async {
        let mut seen = HashSet::with_capacity(request_count);
        while seen.len() < request_count {
            match stream.next().await {
                Some(Ok(event)) if !event.data.trim().is_empty() && event.event != "endpoint" => {
                    let payload: serde_json::Value = serde_json::from_str(&event.data)
                        .expect("SSE event payload was not valid JSON");
                    let id = payload
                        .get("id")
                        .and_then(|value| value.as_str())
                        .expect("SSE payload missing string id")
                        .to_string();
                    seen.insert(id);
                }
                Some(Ok(_)) => {}
                Some(Err(err)) => panic!("SSE stream error while collecting events: {err}"),
                None => panic!("SSE stream ended before all events were received"),
            }
        }
        seen
    };

    tokio::time::timeout(Duration::from_secs(60), sender)
        .await
        .expect("timed out sending high-volume requests to stdio->sse message endpoint");

    let seen = tokio::time::timeout(Duration::from_secs(120), receiver)
        .await
        .unwrap_or_else(|_| {
            panic!(
                "timed out collecting SSE events after send phase; expected {request_count} responses"
            )
        });

    assert_eq!(seen.len(), request_count);

    stop_child(&mut child).await;
}
