mod common;

use std::collections::HashSet;
use std::convert::Infallible;
use std::time::Duration;

use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::sse::Event;
use axum::response::{IntoResponse, Response, Sse};
use axum::routing::post;
use axum::{Json, Router};
use futures::stream;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::oneshot;

use common::{initialize_request, spawn_mcpway, stop_child};

const SESSION_ID: &str = "streamable-concurrency-session";

fn session_header(headers: &HeaderMap) -> Option<String> {
    headers
        .get("Mcp-Session-Id")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
}

fn with_session_header(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert("Mcp-Session-Id", HeaderValue::from_static(SESSION_ID));
    response
}

async fn mcp_post(headers: HeaderMap, Json(payload): Json<serde_json::Value>) -> Response {
    if let Some(inbound_session) = session_header(&headers) {
        assert_eq!(inbound_session, SESSION_ID);
    }

    let method = payload
        .get("method")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    if method == "notifications/initialized" {
        return with_session_header(StatusCode::NO_CONTENT.into_response());
    }

    let id = payload
        .get("id")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    with_session_header(
        Json(serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": { "ok": true }
        }))
        .into_response(),
    )
}

async fn mcp_get(headers: HeaderMap) -> Response {
    if session_header(&headers).as_deref() != Some(SESSION_ID) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let stream = stream::pending::<Result<Event, Infallible>>();
    with_session_header(Sse::new(stream).into_response())
}

#[tokio::test]
async fn streamable_http_to_stdio_handles_request_burst() {
    let app = Router::new()
        .route("/mcp", post(mcp_post).get(mcp_get))
        .with_state(());

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("failed to bind mock streamable HTTP listener");
    let mock_port = listener
        .local_addr()
        .expect("failed to read mock streamable listener address")
        .port();

    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let mock_server = tokio::spawn(async move {
        let server = axum::serve(listener, app.into_make_service()).with_graceful_shutdown(async {
            let _ = shutdown_rx.await;
        });
        let _ = server.await;
    });

    let mut child = spawn_mcpway(
        &[
            "--streamable-http",
            &format!("http://127.0.0.1:{mock_port}/mcp"),
            "--log-level",
            "none",
        ],
        true,
        true,
    )
    .await;

    let request_count = 80usize;
    let stdin = child.stdin.as_mut().expect("stdin was not piped");
    let initialize = initialize_request("streamable-burst-init");
    stdin
        .write_all(format!("{initialize}\n").as_bytes())
        .await
        .expect("failed to write initialize request to stdin");
    for idx in 0..request_count {
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": format!("streamable-burst-{idx}"),
            "method": "tools/list",
            "params": {}
        });
        stdin
            .write_all(format!("{request}\n").as_bytes())
            .await
            .expect("failed to write burst request to stdin");
    }
    stdin.flush().await.expect("failed to flush stdin");

    let stdout = child.stdout.take().expect("stdout was not piped");
    let mut lines = BufReader::new(stdout).lines();
    let total_expected = request_count + 1;
    let mut seen_ids = HashSet::with_capacity(total_expected);
    let deadline = std::time::Instant::now() + Duration::from_secs(40);
    while seen_ids.len() < total_expected {
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for streamable-http burst responses"
        );
        let line = tokio::time::timeout(Duration::from_secs(2), lines.next_line())
            .await
            .expect("timed out waiting for stdout line during burst")
            .expect("failed reading stdout line during burst")
            .expect("stdout closed before all burst responses");
        let payload: serde_json::Value =
            serde_json::from_str(&line).expect("stdout line was not valid JSON");
        let id = payload
            .get("id")
            .and_then(|value| value.as_str())
            .expect("response missing string id")
            .to_string();
        seen_ids.insert(id);
    }

    assert_eq!(seen_ids.len(), total_expected);

    stop_child(&mut child).await;
    let _ = shutdown_tx.send(());
    let _ = mock_server.await;
}
