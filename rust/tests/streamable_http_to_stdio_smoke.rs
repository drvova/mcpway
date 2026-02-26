mod common;

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::sse::Event;
use axum::response::{IntoResponse, Response, Sse};
use axum::routing::post;
use axum::{Json, Router};
use futures::stream;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{oneshot, Mutex};

use common::{initialize_request, spawn_mcpway, stop_child};

const SESSION_ID: &str = "session-1";

#[derive(Clone)]
struct MockState {
    initialized_notification: Arc<Mutex<Option<oneshot::Sender<()>>>>,
}

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

async fn mcp_post(
    State(state): State<MockState>,
    headers: HeaderMap,
    Json(payload): Json<serde_json::Value>,
) -> Response {
    let method = payload
        .get("method")
        .and_then(|value| value.as_str())
        .unwrap_or_default();

    if method == "notifications/initialized" {
        if session_header(&headers).as_deref() == Some(SESSION_ID) {
            if let Some(tx) = state.initialized_notification.lock().await.take() {
                let _ = tx.send(());
            }
        }
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
            "result": {
                "ok": true
            }
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
async fn streamable_http_to_stdio_roundtrip_smoke() {
    let (initialized_tx, initialized_rx) = oneshot::channel::<()>();
    let state = MockState {
        initialized_notification: Arc::new(Mutex::new(Some(initialized_tx))),
    };

    let app = Router::new()
        .route("/mcp", post(mcp_post).get(mcp_get))
        .with_state(state);

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

    let initialize = initialize_request("streamable-inbound-init");
    let stdin = child.stdin.as_mut().expect("stdin was not piped");
    stdin
        .write_all(format!("{initialize}\n").as_bytes())
        .await
        .expect("failed to write initialize request to stdin");
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
    assert_eq!(
        payload.get("id"),
        Some(&serde_json::json!("streamable-inbound-init"))
    );
    assert_eq!(
        payload.get("result").and_then(|result| result.get("ok")),
        Some(&serde_json::json!(true))
    );

    let _ = tokio::time::timeout(Duration::from_secs(10), initialized_rx)
        .await
        .expect("timed out waiting for initialized notification");

    stop_child(&mut child).await;
    let _ = shutdown_tx.send(());
    let _ = mock_server.await;
}
