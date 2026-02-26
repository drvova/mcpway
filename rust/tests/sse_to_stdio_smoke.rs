mod common;

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

async fn sse_handler() -> Sse<impl futures::Stream<Item = Result<Event, Infallible>>> {
    let events = stream::iter(vec![Ok(Event::default()
        .event("endpoint")
        .data("/message"))]);
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
        "result": {
            "ok": true
        }
    }))
    .into_response()
}

#[tokio::test]
async fn sse_to_stdio_roundtrip_smoke() {
    let app = Router::new()
        .route("/sse", get(sse_handler))
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

    let initialize = initialize_request("sse-inbound-init");
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
        Some(&serde_json::json!("sse-inbound-init"))
    );
    assert_eq!(
        payload.get("result").and_then(|result| result.get("ok")),
        Some(&serde_json::json!(true))
    );

    stop_child(&mut child).await;
    let _ = shutdown_tx.send(());
    let _ = mock_server.await;
}
