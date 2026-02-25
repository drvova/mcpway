mod common;

use std::time::Duration;

use common::{find_free_port, initialize_request, spawn_mcpway, stop_child, wait_for_http_status};

#[tokio::test]
async fn stdio_to_streamable_http_stateless_smoke() {
    let port = find_free_port();
    let port_str = port.to_string();

    let mut child = spawn_mcpway(
        &[
            "--stdio",
            "cat",
            "--outputTransport",
            "streamable-http",
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
    let initialize = initialize_request("streamable-stateless-init");

    let post_response = client
        .post(format!("http://127.0.0.1:{port}/mcp"))
        .json(&initialize)
        .send()
        .await
        .expect("failed to POST stateless initialize");
    assert_eq!(post_response.status(), reqwest::StatusCode::OK);

    let post_json: serde_json::Value = post_response
        .json()
        .await
        .expect("stateless POST response was not JSON");
    assert_eq!(
        post_json.get("id"),
        Some(&serde_json::json!("streamable-stateless-init"))
    );

    let get_response = client
        .get(format!("http://127.0.0.1:{port}/mcp"))
        .send()
        .await
        .expect("failed to GET stateless endpoint");
    assert_eq!(
        get_response.status(),
        reqwest::StatusCode::METHOD_NOT_ALLOWED
    );

    let delete_response = client
        .delete(format!("http://127.0.0.1:{port}/mcp"))
        .send()
        .await
        .expect("failed to DELETE stateless endpoint");
    assert_eq!(
        delete_response.status(),
        reqwest::StatusCode::METHOD_NOT_ALLOWED
    );

    stop_child(&mut child).await;
}

#[tokio::test]
async fn stdio_to_streamable_http_stateful_smoke() {
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
    let initialize = initialize_request("streamable-stateful-init");

    let post_response = client
        .post(format!("http://127.0.0.1:{port}/mcp"))
        .json(&initialize)
        .send()
        .await
        .expect("failed to POST stateful initialize");
    assert_eq!(post_response.status(), reqwest::StatusCode::OK);

    let session_id = post_response
        .headers()
        .get("Mcp-Session-Id")
        .or_else(|| post_response.headers().get("mcp-session-id"))
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
        .expect("missing Mcp-Session-Id header on stateful initialize response");

    let post_json: serde_json::Value = post_response
        .json()
        .await
        .expect("stateful POST response was not JSON");
    assert_eq!(
        post_json.get("id"),
        Some(&serde_json::json!("streamable-stateful-init"))
    );

    let get_response = client
        .get(format!("http://127.0.0.1:{port}/mcp"))
        .header("Accept", "text/event-stream")
        .header("Mcp-Session-Id", &session_id)
        .send()
        .await
        .expect("failed to GET stateful SSE stream");
    assert_eq!(get_response.status(), reqwest::StatusCode::OK);
    drop(get_response);

    let delete_response = client
        .delete(format!("http://127.0.0.1:{port}/mcp"))
        .header("Mcp-Session-Id", &session_id)
        .send()
        .await
        .expect("failed to DELETE stateful session");
    assert_eq!(delete_response.status(), reqwest::StatusCode::OK);

    let invalid_get = client
        .get(format!("http://127.0.0.1:{port}/mcp"))
        .header("Accept", "text/event-stream")
        .header("Mcp-Session-Id", &session_id)
        .send()
        .await
        .expect("failed to GET deleted stateful session");
    assert_eq!(invalid_get.status(), reqwest::StatusCode::BAD_REQUEST);

    stop_child(&mut child).await;
}
