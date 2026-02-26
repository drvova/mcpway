mod common;

use std::time::Duration;

use eventsource_stream::Eventsource;
use futures::StreamExt;

use common::{find_free_port, initialize_request, spawn_mcpway, stop_child, wait_for_http_status};

#[tokio::test]
async fn stdio_to_sse_roundtrip_smoke() {
    let port = find_free_port();
    let port_str = port.to_string();

    let mut child = spawn_mcpway(
        &[
            "--stdio",
            "cat",
            "--port",
            &port_str,
            "--sse-path",
            "/sse",
            "--message-path",
            "/message",
            "--health-endpoint",
            "/healthz",
            "--log-level",
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
                Some(Err(err)) => panic!("SSE stream error: {err}"),
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

    let initialize = initialize_request("sse-init");
    let post_response = client
        .post(endpoint_url)
        .json(&initialize)
        .send()
        .await
        .expect("failed to post initialize request");
    assert_eq!(post_response.status(), reqwest::StatusCode::OK);

    let echoed = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match stream.next().await {
                Some(Ok(event)) if !event.data.trim().is_empty() && event.event != "endpoint" => {
                    let parsed: serde_json::Value =
                        serde_json::from_str(&event.data).expect("SSE data was not valid JSON");
                    break parsed;
                }
                Some(Ok(_)) => continue,
                Some(Err(err)) => panic!("SSE stream error: {err}"),
                None => panic!("SSE stream ended before payload event"),
            }
        }
    })
    .await
    .expect("timed out waiting for echoed SSE payload");

    assert_eq!(echoed.get("id"), Some(&serde_json::json!("sse-init")));
    assert_eq!(echoed.get("method"), Some(&serde_json::json!("initialize")));

    stop_child(&mut child).await;
}
