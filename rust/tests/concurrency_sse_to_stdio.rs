mod common;

use std::collections::HashSet;
use std::convert::Infallible;
use std::time::Duration;

use axum::http::StatusCode;
use axum::response::sse::Event;
use axum::response::{IntoResponse, Sse};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures::stream;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::oneshot;

use common::{initialize_request, spawn_mcpway, stop_child};

async fn sse_with_endpoint() -> Sse<impl futures::Stream<Item = Result<Event, Infallible>>> {
    let events = stream::iter(vec![Ok(Event::default()
        .event("endpoint")
        .data("/message"))]);
    Sse::new(events)
}

async fn sse_without_endpoint() -> Sse<impl futures::Stream<Item = Result<Event, Infallible>>> {
    let events = stream::pending::<Result<Event, Infallible>>();
    Sse::new(events)
}

async fn message_handler(Json(payload): Json<serde_json::Value>) -> impl IntoResponse {
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

#[tokio::test]
async fn sse_to_stdio_handles_request_burst() {
    let app = Router::new()
        .route("/sse", get(sse_with_endpoint))
        .route("/message", post(message_handler));

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

    let request_count = 80usize;
    let stdin = child.stdin.as_mut().expect("stdin was not piped");
    for idx in 0..request_count {
        let request = initialize_request(&format!("sse-burst-{idx}"));
        stdin
            .write_all(format!("{request}\n").as_bytes())
            .await
            .expect("failed to write burst request to stdin");
    }
    stdin.flush().await.expect("failed to flush stdin");

    let stdout = child.stdout.take().expect("stdout was not piped");
    let mut lines = BufReader::new(stdout).lines();
    let mut seen_ids = HashSet::with_capacity(request_count);
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while seen_ids.len() < request_count {
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for burst responses"
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

    assert_eq!(seen_ids.len(), request_count);

    stop_child(&mut child).await;
    let _ = shutdown_tx.send(());
    let _ = mock_server.await;
}

#[tokio::test]
async fn sse_to_stdio_errors_when_endpoint_event_is_missing() {
    let app = Router::new().route("/sse", get(sse_without_endpoint));

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

    let request = initialize_request("sse-missing-endpoint");
    let stdin = child.stdin.as_mut().expect("stdin was not piped");
    stdin
        .write_all(format!("{request}\n").as_bytes())
        .await
        .expect("failed to write initialize request to stdin");
    stdin.flush().await.expect("failed to flush stdin");

    let stdout = child.stdout.take().expect("stdout was not piped");
    let mut lines = BufReader::new(stdout).lines();
    let line = tokio::time::timeout(Duration::from_secs(15), lines.next_line())
        .await
        .expect("timed out waiting for timeout error response")
        .expect("failed reading stdout line")
        .expect("stdout closed before timeout error response");
    let payload: serde_json::Value =
        serde_json::from_str(&line).expect("stdout line was not valid JSON");

    assert_eq!(
        payload.get("id"),
        Some(&serde_json::json!("sse-missing-endpoint"))
    );
    assert_eq!(
        payload.pointer("/error/code"),
        Some(&serde_json::json!(-32000))
    );
    let message = payload
        .pointer("/error/message")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    assert!(
        message.contains("Timed out waiting for SSE endpoint"),
        "unexpected error message: {message}"
    );

    stop_child(&mut child).await;
    let _ = shutdown_tx.send(());
    let _ = mock_server.await;
}
