use std::convert::Infallible;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use mcpway::tool_api::{ToolCallError, ToolClientBuilder, Transport};
use serde_json::{json, Value};
use tokio::sync::Mutex;

const SESSION_ID: &str = "tool-api-streamable-session";

#[derive(Clone)]
struct MockState {
    call_count: Arc<AtomicUsize>,
    last_arguments: Arc<Mutex<Option<Value>>>,
}

async fn mcp_post(
    State(state): State<MockState>,
    _headers: HeaderMap,
    Json(payload): Json<Value>,
) -> Response {
    let method = payload
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();

    match method {
        "initialize" => with_session_header(
            Json(json!({
                "jsonrpc": "2.0",
                "id": payload.get("id").cloned().unwrap_or(Value::Null),
                "result": {
                    "protocolVersion": "2024-11-05",
                    "capabilities": {}
                }
            }))
            .into_response(),
        ),
        "notifications/initialized" => with_session_header(StatusCode::NO_CONTENT.into_response()),
        "tools/list" => with_session_header(
            Json(json!({
                "jsonrpc": "2.0",
                "id": payload.get("id").cloned().unwrap_or(Value::Null),
                "result": {
                    "tools": [
                        {
                            "name": "get-weather",
                            "description": "Returns weather",
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
            }))
            .into_response(),
        ),
        "tools/call" => {
            state.call_count.fetch_add(1, Ordering::SeqCst);

            let args = payload
                .pointer("/params/arguments")
                .cloned()
                .unwrap_or(Value::Null);
            *state.last_arguments.lock().await = Some(args.clone());

            with_session_header(
                Json(json!({
                    "jsonrpc": "2.0",
                    "id": payload.get("id").cloned().unwrap_or(Value::Null),
                    "result": {
                        "content": [
                            {"type": "text", "text": "ok"}
                        ],
                        "echo": args
                    }
                }))
                .into_response(),
            )
        }
        _ => with_session_header(
            Json(json!({
                "jsonrpc": "2.0",
                "id": payload.get("id").cloned().unwrap_or(Value::Null),
                "error": {"code": -32601, "message": "method not found"}
            }))
            .into_response(),
        ),
    }
}

fn with_session_header(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert("Mcp-Session-Id", HeaderValue::from_static(SESSION_ID));
    response
}

async fn spawn_mock_server() -> (
    String,
    MockState,
    tokio::task::JoinHandle<Result<(), Infallible>>,
) {
    let state = MockState {
        call_count: Arc::new(AtomicUsize::new(0)),
        last_arguments: Arc::new(Mutex::new(None)),
    };

    let app = Router::new()
        .route("/mcp", post(mcp_post))
        .with_state(state.clone());

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind test listener");
    let addr = listener.local_addr().expect("listener addr");
    let endpoint = format!("http://{}:{}/mcp", addr.ip(), addr.port());

    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve mock app");
        Ok(())
    });

    (endpoint, state, task)
}

#[tokio::test]
async fn streamable_http_applies_defaults_and_calls_tool() {
    let (endpoint, state, server_task) = spawn_mock_server().await;

    let client = ToolClientBuilder::new(endpoint, Transport::StreamableHttp)
        .build()
        .expect("build tool client");

    client.refresh_tools().await.expect("refresh tools");

    let tool = client
        .tools()
        .by_name("get-weather")
        .await
        .expect("resolve tool by canonical name");

    let response = tool
        .call(json!({"city": "Paris"}))
        .await
        .expect("tool call should succeed");

    assert_eq!(
        response
            .pointer("/result/content/0/text")
            .and_then(Value::as_str),
        Some("ok")
    );
    assert_eq!(state.call_count.load(Ordering::SeqCst), 1);

    let last_args = state
        .last_arguments
        .lock()
        .await
        .clone()
        .expect("last arguments should be captured");
    assert_eq!(last_args.get("city").and_then(Value::as_str), Some("Paris"));
    assert_eq!(
        last_args.get("units").and_then(Value::as_str),
        Some("metric")
    );

    server_task.abort();
}

#[tokio::test]
async fn streamable_http_validates_required_before_network_call() {
    let (endpoint, state, server_task) = spawn_mock_server().await;

    let client = ToolClientBuilder::new(endpoint, Transport::StreamableHttp)
        .build()
        .expect("build tool client");

    client.refresh_tools().await.expect("refresh tools");

    let tool = client
        .tools()
        .by_name("get-weather")
        .await
        .expect("resolve tool");

    let err = tool
        .call(json!({}))
        .await
        .expect_err("missing required should fail");

    match err {
        ToolCallError::MissingRequired { key, .. } => assert_eq!(key, "city"),
        other => panic!("unexpected error: {other}"),
    }

    assert_eq!(state.call_count.load(Ordering::SeqCst), 0);
    server_task.abort();
}
