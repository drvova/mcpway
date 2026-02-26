use std::convert::Infallible;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::sse::Event;
use axum::response::{IntoResponse, Response, Sse};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures::stream;
use futures::StreamExt;
use mcpway::tool_api::{ToolClientBuilder, Transport};
use serde_json::{json, Value};
use tokio::sync::{broadcast, Mutex};
use tokio_stream::wrappers::BroadcastStream;

#[derive(Clone)]
struct MockState {
    events: broadcast::Sender<String>,
    call_count: Arc<AtomicUsize>,
    last_arguments: Arc<Mutex<Option<Value>>>,
}

async fn sse_get(State(state): State<MockState>) -> Response {
    let rx = state.events.subscribe();

    let endpoint_event = stream::once(async {
        Ok::<Event, Infallible>(Event::default().event("endpoint").data("/message"))
    });

    let updates = BroadcastStream::new(rx).filter_map(|message| async move {
        match message {
            Ok(payload) => Some(Ok::<Event, Infallible>(Event::default().data(payload))),
            Err(_) => None,
        }
    });

    Sse::new(endpoint_event.chain(updates)).into_response()
}

async fn message_post(
    State(state): State<MockState>,
    Json(payload): Json<Value>,
) -> impl IntoResponse {
    let method = payload
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();

    match method {
        "initialize" => {
            send_event(
                &state,
                json!({
                    "jsonrpc": "2.0",
                    "id": payload.get("id").cloned().unwrap_or(Value::Null),
                    "result": {
                        "protocolVersion": "2024-11-05",
                        "capabilities": {}
                    }
                }),
            );
            StatusCode::ACCEPTED
        }
        "notifications/initialized" => StatusCode::NO_CONTENT,
        "tools/list" => {
            send_event(
                &state,
                json!({
                    "jsonrpc": "2.0",
                    "id": payload.get("id").cloned().unwrap_or(Value::Null),
                    "result": {
                        "tools": [
                            {
                                "name": "get-weather",
                                "inputSchema": {
                                    "type": "object",
                                    "properties": {
                                        "city": {"type": "string"},
                                        "units": {"type": "string", "default": "metric"}
                                    },
                                    "required": ["city"]
                                }
                            }
                        ]
                    }
                }),
            );
            StatusCode::ACCEPTED
        }
        "tools/call" => {
            state.call_count.fetch_add(1, Ordering::SeqCst);
            let args = payload
                .pointer("/params/arguments")
                .cloned()
                .unwrap_or(Value::Null);
            *state.last_arguments.lock().await = Some(args.clone());

            send_event(
                &state,
                json!({
                    "jsonrpc": "2.0",
                    "id": payload.get("id").cloned().unwrap_or(Value::Null),
                    "result": {
                        "content": [{"type": "text", "text": "ok"}],
                        "echo": args
                    }
                }),
            );
            StatusCode::ACCEPTED
        }
        _ => {
            send_event(
                &state,
                json!({
                    "jsonrpc": "2.0",
                    "id": payload.get("id").cloned().unwrap_or(Value::Null),
                    "error": {"code": -32601, "message": "method not found"}
                }),
            );
            StatusCode::ACCEPTED
        }
    }
}

fn send_event(state: &MockState, payload: Value) {
    let _ = state.events.send(payload.to_string());
}

async fn spawn_mock_server() -> (String, MockState, tokio::task::JoinHandle<()>) {
    let (events_tx, _) = broadcast::channel(32);
    let state = MockState {
        events: events_tx,
        call_count: Arc::new(AtomicUsize::new(0)),
        last_arguments: Arc::new(Mutex::new(None)),
    };

    let app = Router::new()
        .route("/sse", get(sse_get))
        .route("/message", post(message_post))
        .with_state(state.clone());

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind test listener");
    let addr = listener.local_addr().expect("listener addr");
    let endpoint = format!("http://{}:{}/sse", addr.ip(), addr.port());

    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve mock app");
    });

    (endpoint, state, task)
}

#[tokio::test]
async fn sse_transport_receives_async_events_and_calls_tools() {
    let (endpoint, state, server_task) = spawn_mock_server().await;

    let client = ToolClientBuilder::new(endpoint, Transport::Sse)
        .build()
        .expect("build tool client");

    client.refresh_tools().await.expect("refresh tools");

    let tool = client
        .tools()
        .by_name("get-weather")
        .await
        .expect("resolve tool by canonical name");

    let response = tool
        .call(json!({"city": "Tokyo"}))
        .await
        .expect("tool call should succeed");

    assert_eq!(
        response
            .pointer("/result/content/0/text")
            .and_then(Value::as_str),
        Some("ok")
    );
    assert_eq!(state.call_count.load(Ordering::SeqCst), 1);

    let args = state
        .last_arguments
        .lock()
        .await
        .clone()
        .expect("captured tool arguments");
    assert_eq!(args.get("city").and_then(Value::as_str), Some("Tokyo"));
    assert_eq!(args.get("units").and_then(Value::as_str), Some("metric"));

    server_task.abort();
}
