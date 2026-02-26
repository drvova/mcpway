use std::convert::Infallible;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderValue, Response, StatusCode};
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use mcpway::tool_api::{ToolClientBuilder, Transport};
use serde::Serialize;
use serde_json::{json, Value};
use tokio::sync::Mutex;

const SESSION_ID: &str = "tool-api-ergonomic-session";

#[derive(Clone)]
struct MockState {
    call_count: Arc<AtomicUsize>,
    last_arguments: Arc<Mutex<Option<Value>>>,
}

async fn mcp_post(
    State(state): State<MockState>,
    Json(payload): Json<Value>,
) -> Result<Response<axum::body::Body>, Infallible> {
    let method = payload
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();

    let mut response = match method {
        "initialize" => Json(json!({
            "jsonrpc": "2.0",
            "id": payload.get("id").cloned().unwrap_or(Value::Null),
            "result": {
                "protocolVersion": "2024-11-05",
                "capabilities": {}
            }
        }))
        .into_response(),
        "notifications/initialized" => StatusCode::NO_CONTENT.into_response(),
        "tools/list" => Json(json!({
            "jsonrpc": "2.0",
            "id": payload.get("id").cloned().unwrap_or(Value::Null),
            "result": {
                "tools": [
                    {
                        "name": "get-weather-report",
                        "description": "Weather lookup",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "city": {"type": "string"},
                                "units": {"type": "string", "default": "metric"},
                                "prefs": {
                                    "type": "object",
                                    "properties": {
                                        "lang": {"type": "string", "default": "en"}
                                    }
                                }
                            },
                            "required": ["city"]
                        }
                    }
                ]
            }
        }))
        .into_response(),
        "tools/call" => {
            state.call_count.fetch_add(1, Ordering::SeqCst);
            let args = payload
                .pointer("/params/arguments")
                .cloned()
                .unwrap_or(Value::Null);
            *state.last_arguments.lock().await = Some(args.clone());
            Json(json!({
                "jsonrpc": "2.0",
                "id": payload.get("id").cloned().unwrap_or(Value::Null),
                "result": {
                    "content": [{"type": "text", "text": "ok"}],
                    "echo": args
                }
            }))
            .into_response()
        }
        _ => Json(json!({
            "jsonrpc": "2.0",
            "id": payload.get("id").cloned().unwrap_or(Value::Null),
            "error": {"code": -32601, "message": "method not found"}
        }))
        .into_response(),
    };

    response
        .headers_mut()
        .insert("Mcp-Session-Id", HeaderValue::from_static(SESSION_ID));
    Ok(response)
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
        .expect("bind listener");
    let addr = listener.local_addr().expect("listener addr");
    let endpoint = format!("http://{}:{}/mcp", addr.ip(), addr.port());
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve mock app");
        Ok(())
    });

    (endpoint, state, task)
}

#[derive(Serialize)]
struct WeatherRequest {
    city: String,
}

#[tokio::test]
async fn ergonomic_facade_supports_canonical_introspection_and_typed_calls() {
    let (endpoint, state, server_task) = spawn_mock_server().await;
    let client = ToolClientBuilder::new(endpoint, Transport::StreamableHttp)
        .build()
        .expect("build client");

    let ergonomic = client.ergonomic();
    let tools = ergonomic.list().await.expect("list tools should succeed");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "get-weather-report");
    assert_eq!(tools[0].required_keys, 1);
    assert_eq!(tools[0].defaulted_keys, 2);

    let prepared = ergonomic
        .prepare_args("get-weather-report", json!({"city":"Berlin","prefs":{}}))
        .await
        .expect("prepare args should succeed");
    assert_eq!(prepared["units"], json!("metric"));
    assert_eq!(prepared["prefs"]["lang"], json!("en"));

    let response = ergonomic
        .call_struct(
            "get-weather-report",
            &WeatherRequest {
                city: "Berlin".to_string(),
            },
        )
        .await
        .expect("typed call should succeed");
    assert_eq!(
        response
            .pointer("/result/content/0/text")
            .and_then(Value::as_str),
        Some("ok")
    );

    let captured_args = state
        .last_arguments
        .lock()
        .await
        .clone()
        .expect("args should be captured");
    assert_eq!(captured_args["city"], json!("Berlin"));
    assert_eq!(captured_args["units"], json!("metric"));
    assert_eq!(state.call_count.load(Ordering::SeqCst), 1);

    server_task.abort();
}
