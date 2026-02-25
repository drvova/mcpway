use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::sse::Event;
use axum::response::{IntoResponse, Response, Sse};
use axum::routing::{get, post};
use axum::{Json, Router};
use tokio::sync::{mpsc, Mutex};
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::StreamExt;
use uuid::Uuid;

use crate::config::Config;
use crate::runtime::store::RuntimeArgsStore;
use crate::runtime::{RuntimeApplyResult, RuntimeScope, RuntimeUpdateRequest};
use crate::support::command_spec::parse_command_spec;
use crate::support::cors::build_cors_layer;
use crate::support::signals::install_signal_handlers;
use crate::support::stdio_child::StdioChild;

const SESSION_EVENT_BUFFER: usize = 256;

#[derive(Clone)]
struct AppState {
    sessions: Arc<Mutex<HashMap<String, mpsc::Sender<Event>>>>,
    child: Arc<StdioChild>,
    runtime: RuntimeArgsStore,
    base_headers: HeaderMap,
    message_path: String,
    base_url: String,
}

#[derive(serde::Deserialize)]
struct MessageQuery {
    #[serde(rename = "sessionId")]
    session_id: String,
}

pub async fn run(
    config: Config,
    runtime: RuntimeArgsStore,
    mut updates: mpsc::Receiver<RuntimeUpdateRequest>,
) -> Result<(), String> {
    let stdio_cmd = config.stdio.clone().ok_or("stdio command is required")?;
    tracing::info!(
        "  - Headers: {}",
        serde_json::to_string(&config.headers).unwrap_or_else(|_| "(none)".into())
    );
    tracing::info!("  - port: {}", config.port);
    tracing::info!("  - stdio: {}", stdio_cmd);
    if !config.base_url.is_empty() {
        tracing::info!("  - baseUrl: {}", config.base_url);
    }
    tracing::info!("  - ssePath: {}", config.sse_path);
    tracing::info!("  - messagePath: {}", config.message_path);

    let spec = parse_command_spec(&stdio_cmd)?;
    let child = Arc::new(StdioChild::new(spec, true));
    let initial_args = runtime.get_effective(None).await;
    child.spawn(&initial_args).await?;

    let sessions: Arc<Mutex<HashMap<String, mpsc::Sender<Event>>>> =
        Arc::new(Mutex::new(HashMap::new()));

    let state = AppState {
        sessions: sessions.clone(),
        child: child.clone(),
        runtime: runtime.clone(),
        base_headers: header_map_from(&config.headers),
        message_path: config.message_path.clone(),
        base_url: config.base_url.clone(),
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
                    "Per-session runtime overrides are not supported for stdio→SSE",
                ),
            };
            let _ = req.respond_to.send(result);
        }
    });

    let mut router = Router::new()
        .route(&config.sse_path, get(sse_handler))
        .route(&config.message_path, post(message_handler))
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
        while let Ok(msg) = rx.recv().await {
            let json = match serde_json::to_string(&msg) {
                Ok(val) => val,
                Err(_) => continue,
            };
            let event = Event::default().data(json);

            let senders: Vec<(String, mpsc::Sender<Event>)> = {
                let sessions_guard = sessions.lock().await;
                sessions_guard
                    .iter()
                    .map(|(id, sender)| (id.clone(), sender.clone()))
                    .collect()
            };

            let mut dead = Vec::new();
            for (id, sender) in senders {
                if sender.send(event.clone()).await.is_err() {
                    dead.push(id);
                }
            }

            if !dead.is_empty() {
                let mut sessions_guard = sessions.lock().await;
                for id in dead {
                    sessions_guard.remove(&id);
                }
            }
        }
    });

    let addr: std::net::SocketAddr = ([0, 0, 0, 0], config.port).into();
    tracing::info!("Listening on port {}", config.port);
    tracing::info!(
        "SSE endpoint: http://localhost:{}{}",
        config.port,
        config.sse_path
    );
    tracing::info!(
        "POST messages: http://localhost:{}{}",
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

async fn sse_handler(State(state): State<AppState>) -> Response {
    let session_id = Uuid::new_v4().to_string();
    let (tx, rx) = mpsc::channel(SESSION_EVENT_BUFFER);
    {
        let mut sessions = state.sessions.lock().await;
        sessions.insert(session_id.clone(), tx.clone());
    }

    let endpoint = if state.base_url.is_empty() {
        format!("{}?sessionId={}", state.message_path, session_id)
    } else {
        format!(
            "{}{}?sessionId={}",
            state.base_url, state.message_path, session_id
        )
    };
    let _ = tx
        .send(Event::default().event("endpoint").data(endpoint))
        .await;

    let stream = ReceiverStream::new(rx).map(Ok::<Event, std::convert::Infallible>);
    let sse = Sse::new(stream).keep_alive(axum::response::sse::KeepAlive::default());
    let mut response = sse.into_response();
    apply_headers(&state, &mut response).await;
    response
}

async fn message_handler(
    State(state): State<AppState>,
    Query(query): Query<MessageQuery>,
    Json(payload): Json<serde_json::Value>,
) -> impl IntoResponse {
    if query.session_id.is_empty() {
        let mut response = (StatusCode::BAD_REQUEST, "Missing sessionId parameter").into_response();
        apply_headers(&state, &mut response).await;
        return response;
    }

    let session_active = {
        let mut sessions = state.sessions.lock().await;
        match sessions.get(&query.session_id) {
            Some(sender) if !sender.is_closed() => true,
            Some(_) => {
                sessions.remove(&query.session_id);
                false
            }
            None => false,
        }
    };
    if !session_active {
        let mut response = (
            StatusCode::SERVICE_UNAVAILABLE,
            format!("No active SSE connection for session {}", query.session_id),
        )
            .into_response();
        apply_headers(&state, &mut response).await;
        return response;
    }

    if state.child.send(&payload).await.is_err() {
        let mut response = (StatusCode::BAD_GATEWAY, "Failed to write to child").into_response();
        apply_headers(&state, &mut response).await;
        return response;
    }

    let mut response = StatusCode::OK.into_response();
    apply_headers(&state, &mut response).await;
    response
}

async fn health_handler(state: AppState) -> impl IntoResponse {
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
