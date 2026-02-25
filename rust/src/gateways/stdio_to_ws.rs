use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use futures::{SinkExt, StreamExt};
use tokio::sync::{mpsc, Mutex};
use uuid::Uuid;

use crate::config::Config;
use crate::runtime::store::RuntimeArgsStore;
use crate::runtime::{RuntimeApplyResult, RuntimeScope, RuntimeUpdateRequest};
use crate::support::command_spec::parse_command_spec;
use crate::support::cors::build_cors_layer;
use crate::support::signals::install_signal_handlers;
use crate::support::stdio_child::StdioChild;

const WS_CLIENT_BUFFER: usize = 256;

#[derive(Clone)]
struct AppState {
    clients: Arc<Mutex<HashMap<String, mpsc::Sender<serde_json::Value>>>>,
    child: Arc<StdioChild>,
    runtime: RuntimeArgsStore,
    base_headers: HeaderMap,
    ready: Arc<AtomicBool>,
}

pub async fn run(
    config: Config,
    runtime: RuntimeArgsStore,
    mut updates: mpsc::Receiver<RuntimeUpdateRequest>,
) -> Result<(), String> {
    let stdio_cmd = config.stdio.clone().ok_or("stdio command is required")?;

    tracing::info!("  - port: {}", config.port);
    tracing::info!("  - stdio: {}", stdio_cmd);
    tracing::info!("  - messagePath: {}", config.message_path);

    let spec = parse_command_spec(&stdio_cmd)?;
    let child = Arc::new(StdioChild::new(spec, true));
    let initial_args = runtime.get_effective(None).await;
    child.spawn(&initial_args).await?;

    let clients: Arc<Mutex<HashMap<String, mpsc::Sender<serde_json::Value>>>> =
        Arc::new(Mutex::new(HashMap::new()));

    let ready = Arc::new(AtomicBool::new(false));
    let state = AppState {
        clients: clients.clone(),
        child: child.clone(),
        runtime: runtime.clone(),
        base_headers: header_map_from(&config.headers),
        ready: ready.clone(),
    };

    let runtime_child = child.clone();
    let runtime_store = runtime.clone();
    tokio::spawn(async move {
        while let Some(req) = updates.recv().await {
            let result = match req.update.scope {
                RuntimeScope::Global => {
                    let update_result = runtime_store.update_global(req.update.update).await;
                    if update_result.restart_needed {
                        let args = runtime_store.get_effective(None).await;
                        if runtime_child.restart(&args).await.is_err() {
                            RuntimeApplyResult::error("Failed to restart child")
                        } else {
                            RuntimeApplyResult::ok("Restarted child with new runtime args", true)
                        }
                    } else {
                        RuntimeApplyResult::ok("Updated runtime args", false)
                    }
                }
                RuntimeScope::Session(_) => RuntimeApplyResult::error(
                    "Per-session runtime overrides are not supported for stdio→WS",
                ),
            };
            let _ = req.respond_to.send(result);
        }
    });

    let mut router = Router::new()
        .route(&config.message_path, get(ws_handler))
        .with_state(state.clone());

    for ep in &config.health_endpoints {
        let state = state.clone();
        let path = ep.clone();
        router = router.route(
            &path,
            get(move || async move { health_handler(state.clone()).await }),
        );
    }

    if let Some(cors) = build_cors_layer(&config.cors) {
        router = router.layer(cors);
    }

    install_signal_handlers(None);

    let mut rx = child.subscribe();
    tokio::spawn(async move {
        loop {
            let msg = match rx.recv().await {
                Ok(msg) => msg,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    tracing::warn!(
                        "stdio->ws child output receiver lagged by {skipped} messages; continuing"
                    );
                    continue;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            };

            let mut target_id: Option<String> = None;
            let mut outgoing = msg.clone();
            if let Some((client_id, raw_id)) = strip_prefixed_id(&msg) {
                target_id = Some(client_id);
                if let Some(obj) = outgoing.as_object_mut() {
                    obj.insert("id".to_string(), raw_id);
                }
            }

            if let Some(target) = target_id {
                let sender = {
                    let clients_guard = clients.lock().await;
                    clients_guard.get(&target).cloned()
                };
                if let Some(sender) = sender {
                    if sender.send(outgoing).await.is_err() {
                        let mut clients_guard = clients.lock().await;
                        clients_guard.remove(&target);
                    }
                }
                continue;
            }

            let senders: Vec<(String, mpsc::Sender<serde_json::Value>)> = {
                let clients_guard = clients.lock().await;
                clients_guard
                    .iter()
                    .map(|(id, sender)| (id.clone(), sender.clone()))
                    .collect()
            };

            let mut dead = Vec::new();
            for (id, sender) in senders {
                if sender.send(outgoing.clone()).await.is_err() {
                    dead.push(id);
                }
            }
            if !dead.is_empty() {
                let mut clients_guard = clients.lock().await;
                for id in dead {
                    clients_guard.remove(&id);
                }
            }
        }
    });

    ready.store(true, Ordering::SeqCst);

    let addr: std::net::SocketAddr = ([0, 0, 0, 0], config.port).into();
    tracing::info!("Listening on port {}", config.port);
    tracing::info!(
        "WebSocket endpoint: ws://localhost:{}{}",
        config.port,
        config.message_path
    );

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|err| err.to_string())?;
    axum::serve(listener, router.into_make_service())
        .await
        .map_err(|err| err.to_string())
}

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(stream: WebSocket, state: AppState) {
    let client_id = Uuid::new_v4().to_string();
    let (mut sender_ws, mut receiver_ws) = stream.split();
    let (tx, mut rx) = mpsc::channel::<serde_json::Value>(WS_CLIENT_BUFFER);

    {
        let mut clients = state.clients.lock().await;
        clients.insert(client_id.clone(), tx);
    }

    let child = state.child.clone();
    let send_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if let Ok(text) = serde_json::to_string(&msg) {
                if sender_ws.send(Message::Text(text.into())).await.is_err() {
                    break;
                }
            }
        }
    });

    let client_id_clone = client_id.clone();
    let recv_task = tokio::spawn(async move {
        while let Some(Ok(message)) = receiver_ws.next().await {
            if let Message::Text(text) = message {
                if let Ok(mut json) = serde_json::from_str::<serde_json::Value>(&text) {
                    if let Some(id) = json.get("id").cloned() {
                        let prefixed = prefix_id(&client_id_clone, &id);
                        if let Some(obj) = json.as_object_mut() {
                            obj.insert("id".to_string(), prefixed);
                        }
                    }
                    let _ = child.send(&json).await;
                }
            }
        }
    });

    let _ = tokio::join!(send_task, recv_task);

    let mut clients = state.clients.lock().await;
    clients.remove(&client_id);
}

async fn health_handler(state: AppState) -> impl IntoResponse {
    if !state.child.is_alive().await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Child process has been killed",
        )
            .into_response();
    }
    if !state.ready.load(Ordering::SeqCst) {
        return (StatusCode::INTERNAL_SERVER_ERROR, "Server is not ready").into_response();
    }
    let mut response = (StatusCode::OK, "ok").into_response();
    apply_headers(&state, &mut response).await;
    response
}

async fn apply_headers(state: &AppState, response: &mut Response) {
    let runtime = state.runtime.get_effective(None).await;
    let headers = merge_headers(&state.base_headers, &runtime.headers);
    let header_map = response.headers_mut();
    for (key, value) in headers.iter() {
        header_map.insert(key, value.clone());
    }
}

fn header_map_from(headers: &std::collections::HashMap<String, String>) -> HeaderMap {
    let mut map = HeaderMap::new();
    for (k, v) in headers {
        if let (Ok(name), Ok(value)) = (
            axum::http::header::HeaderName::from_bytes(k.as_bytes()),
            HeaderValue::from_str(v),
        ) {
            map.insert(name, value);
        }
    }
    map
}

fn merge_headers(
    base: &HeaderMap,
    overlay: &std::collections::HashMap<String, String>,
) -> HeaderMap {
    let mut merged = base.clone();
    for (k, v) in overlay {
        if let (Ok(name), Ok(value)) = (
            axum::http::header::HeaderName::from_bytes(k.as_bytes()),
            HeaderValue::from_str(v),
        ) {
            merged.insert(name, value);
        }
    }
    merged
}

fn prefix_id(client_id: &str, id: &serde_json::Value) -> serde_json::Value {
    match id {
        serde_json::Value::String(s) => serde_json::Value::String(format!("{client_id}:{s}")),
        serde_json::Value::Number(n) => serde_json::Value::String(format!("{client_id}:{n}")),
        _ => serde_json::Value::String(format!("{client_id}:{id}")),
    }
}

fn strip_prefixed_id(message: &serde_json::Value) -> Option<(String, serde_json::Value)> {
    let id_val = message.get("id")?;
    let id_str = id_val.as_str()?;
    let mut parts = id_str.splitn(2, ':');
    let client = parts.next()?.to_string();
    let raw = parts.next()?.to_string();
    let raw_id = raw
        .parse::<i64>()
        .map(|num| serde_json::Value::Number(num.into()))
        .unwrap_or_else(|_| serde_json::Value::String(raw));
    Some((client, raw_id))
}
