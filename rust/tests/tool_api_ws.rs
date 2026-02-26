use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use futures::{SinkExt, StreamExt};
use mcpway::tool_api::{ToolClientBuilder, Transport};
use serde_json::{json, Value};
use tokio::sync::Mutex;
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::Message;

#[derive(Clone)]
struct MockState {
    call_count: Arc<AtomicUsize>,
    last_arguments: Arc<Mutex<Option<Value>>>,
}

async fn spawn_mock_server() -> (String, MockState, tokio::task::JoinHandle<()>) {
    let state = MockState {
        call_count: Arc::new(AtomicUsize::new(0)),
        last_arguments: Arc::new(Mutex::new(None)),
    };

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind websocket listener");
    let addr = listener.local_addr().expect("listener addr");
    let endpoint = format!("ws://{}:{}/ws", addr.ip(), addr.port());

    let server_state = state.clone();
    let task = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept ws socket");
        let mut ws = accept_async(stream).await.expect("accept websocket");

        while let Some(frame) = ws.next().await {
            let frame = frame.expect("read ws frame");
            let text = match frame {
                Message::Text(text) => text.to_string(),
                Message::Binary(bytes) => String::from_utf8(bytes.to_vec()).expect("utf8 binary"),
                Message::Close(_) => break,
                Message::Ping(payload) => {
                    ws.send(Message::Pong(payload)).await.expect("send pong");
                    continue;
                }
                Message::Pong(_) => continue,
                _ => continue,
            };

            let payload: Value = serde_json::from_str(&text).expect("json payload");
            let method = payload
                .get("method")
                .and_then(Value::as_str)
                .unwrap_or_default();

            match method {
                "initialize" => {
                    let response = json!({
                        "jsonrpc": "2.0",
                        "id": payload.get("id").cloned().unwrap_or(Value::Null),
                        "result": {
                            "protocolVersion": "2024-11-05",
                            "capabilities": {}
                        }
                    });
                    ws.send(Message::Text(response.to_string().into()))
                        .await
                        .expect("send initialize response");
                }
                "notifications/initialized" => {}
                "tools/list" => {
                    let response = json!({
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
                    });
                    ws.send(Message::Text(response.to_string().into()))
                        .await
                        .expect("send tools list response");
                }
                "tools/call" => {
                    server_state.call_count.fetch_add(1, Ordering::SeqCst);

                    let args = payload
                        .pointer("/params/arguments")
                        .cloned()
                        .unwrap_or(Value::Null);
                    *server_state.last_arguments.lock().await = Some(args.clone());

                    let response = json!({
                        "jsonrpc": "2.0",
                        "id": payload.get("id").cloned().unwrap_or(Value::Null),
                        "result": {
                            "content": [{"type": "text", "text": "ok"}],
                            "echo": args
                        }
                    });
                    ws.send(Message::Text(response.to_string().into()))
                        .await
                        .expect("send tool call response");
                }
                _ => {
                    let response = json!({
                        "jsonrpc": "2.0",
                        "id": payload.get("id").cloned().unwrap_or(Value::Null),
                        "error": {"code": -32601, "message": "method not found"}
                    });
                    ws.send(Message::Text(response.to_string().into()))
                        .await
                        .expect("send error response");
                }
            }
        }
    });

    (endpoint, state, task)
}

#[tokio::test]
async fn websocket_transport_calls_tools_with_schema_defaults() {
    let (endpoint, state, server_task) = spawn_mock_server().await;

    let client = ToolClientBuilder::new(endpoint, Transport::Ws)
        .build()
        .expect("build tool client");

    client.refresh_tools().await.expect("refresh tools");

    let tool = client
        .tools()
        .by_name("get-weather")
        .await
        .expect("resolve tool by canonical name");

    let response = tool
        .call(json!({"city": "Seoul"}))
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
    assert_eq!(args.get("city").and_then(Value::as_str), Some("Seoul"));
    assert_eq!(args.get("units").and_then(Value::as_str), Some("metric"));

    server_task.abort();
}
