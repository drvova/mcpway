mod common;

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::sse::Event;
use axum::response::{IntoResponse, Response, Sse};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures::stream;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{oneshot, Mutex};

use common::{spawn_mcpway, stop_child, wait_for_condition};

#[derive(Clone)]
struct RecordedState {
    messages: Arc<Mutex<Vec<serde_json::Value>>>,
}

async fn sse_handler() -> Sse<impl futures::Stream<Item = Result<Event, Infallible>>> {
    let events = stream::iter(vec![Ok(Event::default()
        .event("endpoint")
        .data("/message"))]);
    Sse::new(events)
}

async fn sse_message_handler(
    State(state): State<RecordedState>,
    Json(payload): Json<serde_json::Value>,
) -> impl IntoResponse {
    state.messages.lock().await.push(payload.clone());

    let method = payload
        .get("method")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    if method == "notifications/initialized" {
        return StatusCode::NO_CONTENT.into_response();
    }

    let id = payload
        .get("id")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    Json(serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": { "ok": true }
    }))
    .into_response()
}

const SESSION_ID: &str = "protocol-session-1";

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

async fn streamable_post_handler(
    State(state): State<RecordedState>,
    Json(payload): Json<serde_json::Value>,
) -> Response {
    state.messages.lock().await.push(payload.clone());

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

async fn streamable_get_handler(headers: HeaderMap) -> Response {
    if session_header(&headers).as_deref() != Some(SESSION_ID) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let stream = stream::pending::<Result<Event, Infallible>>();
    with_session_header(Sse::new(stream).into_response())
}

async fn write_stdin_and_read_one_response(
    child: &mut tokio::process::Child,
    request: &serde_json::Value,
) -> serde_json::Value {
    let stdin = child.stdin.as_mut().expect("stdin was not piped");
    stdin
        .write_all(format!("{request}\n").as_bytes())
        .await
        .expect("failed to write request to stdin");
    stdin.flush().await.expect("failed to flush stdin");

    let stdout = child.stdout.take().expect("stdout was not piped");
    let mut lines = BufReader::new(stdout).lines();
    let line = tokio::time::timeout(Duration::from_secs(10), lines.next_line())
        .await
        .expect("timed out waiting for stdout response")
        .expect("failed reading stdout line")
        .expect("stdout closed before response line");
    serde_json::from_str(&line).expect("stdout line was not valid JSON")
}

#[tokio::test]
async fn sse_to_stdio_preserves_client_protocol_version_on_initialize() {
    let state = RecordedState {
        messages: Arc::new(Mutex::new(Vec::new())),
    };
    let app = Router::new()
        .route("/sse", get(sse_handler))
        .route("/message", post(sse_message_handler))
        .with_state(state.clone());

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("failed to bind mock SSE listener");
    let mock_port = listener
        .local_addr()
        .expect("failed to read mock SSE listener address")
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
            "--sse",
            &format!("http://127.0.0.1:{mock_port}/sse"),
            "--log-level",
            "none",
        ],
        true,
        true,
    )
    .await;

    let protocol_version = "2025-03-26";
    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": "init-sse-parity",
        "method": "initialize",
        "params": {
            "protocolVersion": protocol_version,
            "capabilities": {},
            "clientInfo": { "name": "test-client", "version": "1.0.0" }
        }
    });

    let response = write_stdin_and_read_one_response(&mut child, &request).await;
    assert_eq!(
        response.get("id"),
        Some(&serde_json::json!("init-sse-parity"))
    );

    wait_for_condition(Duration::from_secs(5), || {
        let messages = state.messages.clone();
        async move { !messages.lock().await.is_empty() }
    })
    .await;

    let messages = state.messages.lock().await;
    let initialize = messages
        .iter()
        .find(|msg| msg.get("method") == Some(&serde_json::json!("initialize")))
        .expect("expected initialize request to be sent upstream");
    assert_eq!(
        initialize
            .get("params")
            .and_then(|params| params.get("protocolVersion")),
        Some(&serde_json::json!(protocol_version))
    );

    stop_child(&mut child).await;
    let _ = shutdown_tx.send(());
    let _ = mock_server.await;
}

#[tokio::test]
async fn sse_to_stdio_auto_initialize_uses_configured_protocol_version() {
    let state = RecordedState {
        messages: Arc::new(Mutex::new(Vec::new())),
    };
    let app = Router::new()
        .route("/sse", get(sse_handler))
        .route("/message", post(sse_message_handler))
        .with_state(state.clone());

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("failed to bind mock SSE listener");
    let mock_port = listener
        .local_addr()
        .expect("failed to read mock SSE listener address")
        .port();

    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let mock_server = tokio::spawn(async move {
        let server = axum::serve(listener, app.into_make_service()).with_graceful_shutdown(async {
            let _ = shutdown_rx.await;
        });
        let _ = server.await;
    });

    let configured_protocol = "2099-01-01";
    let mut child = spawn_mcpway(
        &[
            "--sse",
            &format!("http://127.0.0.1:{mock_port}/sse"),
            "--protocol-version",
            configured_protocol,
            "--log-level",
            "none",
        ],
        true,
        true,
    )
    .await;

    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": "auto-sse-parity",
        "method": "tools/list",
        "params": {}
    });
    let response = write_stdin_and_read_one_response(&mut child, &request).await;
    assert_eq!(
        response.get("id"),
        Some(&serde_json::json!("auto-sse-parity"))
    );

    wait_for_condition(Duration::from_secs(5), || {
        let messages = state.messages.clone();
        async move {
            let recorded = messages.lock().await;
            recorded
                .iter()
                .any(|msg| msg.get("method") == Some(&serde_json::json!("initialize")))
        }
    })
    .await;

    let messages = state.messages.lock().await;
    let initialize = messages
        .iter()
        .find(|msg| msg.get("method") == Some(&serde_json::json!("initialize")))
        .expect("expected auto initialize request");
    assert_eq!(
        initialize
            .get("params")
            .and_then(|params| params.get("protocolVersion")),
        Some(&serde_json::json!(configured_protocol))
    );

    stop_child(&mut child).await;
    let _ = shutdown_tx.send(());
    let _ = mock_server.await;
}

#[tokio::test]
async fn streamable_http_to_stdio_preserves_client_protocol_version_on_initialize() {
    let state = RecordedState {
        messages: Arc::new(Mutex::new(Vec::new())),
    };
    let app = Router::new()
        .route(
            "/mcp",
            post(streamable_post_handler).get(streamable_get_handler),
        )
        .with_state(state.clone());

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("failed to bind mock streamable listener");
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

    let protocol_version = "2025-12-31";
    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": "init-streamable-parity",
        "method": "initialize",
        "params": {
            "protocolVersion": protocol_version,
            "capabilities": {},
            "clientInfo": { "name": "test-client", "version": "1.0.0" }
        }
    });

    let response = write_stdin_and_read_one_response(&mut child, &request).await;
    assert_eq!(
        response.get("id"),
        Some(&serde_json::json!("init-streamable-parity"))
    );

    wait_for_condition(Duration::from_secs(5), || {
        let messages = state.messages.clone();
        async move { !messages.lock().await.is_empty() }
    })
    .await;

    let messages = state.messages.lock().await;
    let initialize = messages
        .iter()
        .find(|msg| msg.get("method") == Some(&serde_json::json!("initialize")))
        .expect("expected initialize request to be sent upstream");
    assert_eq!(
        initialize
            .get("params")
            .and_then(|params| params.get("protocolVersion")),
        Some(&serde_json::json!(protocol_version))
    );

    stop_child(&mut child).await;
    let _ = shutdown_tx.send(());
    let _ = mock_server.await;
}

#[tokio::test]
async fn streamable_http_to_stdio_auto_initialize_uses_configured_protocol_version() {
    let state = RecordedState {
        messages: Arc::new(Mutex::new(Vec::new())),
    };
    let app = Router::new()
        .route(
            "/mcp",
            post(streamable_post_handler).get(streamable_get_handler),
        )
        .with_state(state.clone());

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("failed to bind mock streamable listener");
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

    let configured_protocol = "2098-02-02";
    let mut child = spawn_mcpway(
        &[
            "--streamable-http",
            &format!("http://127.0.0.1:{mock_port}/mcp"),
            "--protocol-version",
            configured_protocol,
            "--log-level",
            "none",
        ],
        true,
        true,
    )
    .await;

    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": "auto-streamable-parity",
        "method": "tools/list",
        "params": {}
    });
    let response = write_stdin_and_read_one_response(&mut child, &request).await;
    assert_eq!(
        response.get("id"),
        Some(&serde_json::json!("auto-streamable-parity"))
    );

    wait_for_condition(Duration::from_secs(5), || {
        let messages = state.messages.clone();
        async move {
            let recorded = messages.lock().await;
            recorded
                .iter()
                .any(|msg| msg.get("method") == Some(&serde_json::json!("initialize")))
        }
    })
    .await;

    let messages = state.messages.lock().await;
    let initialize = messages
        .iter()
        .find(|msg| msg.get("method") == Some(&serde_json::json!("initialize")))
        .expect("expected auto initialize request");
    assert_eq!(
        initialize
            .get("params")
            .and_then(|params| params.get("protocolVersion")),
        Some(&serde_json::json!(configured_protocol))
    );

    stop_child(&mut child).await;
    let _ = shutdown_tx.send(());
    let _ = mock_server.await;
}
