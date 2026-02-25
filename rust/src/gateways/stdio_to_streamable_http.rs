use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::sse::Event;
use axum::response::{IntoResponse, Response, Sse};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use futures::StreamExt;
use tokio::sync::{broadcast, mpsc, oneshot, Mutex};
use tokio_stream::wrappers::BroadcastStream;
use uuid::Uuid;

use crate::config::Config;
use crate::runtime::store::RuntimeArgsStore;
use crate::runtime::{RuntimeApplyResult, RuntimeScope, RuntimeUpdateRequest};
use crate::support::command_spec::parse_command_spec;
use crate::support::cors::build_cors_layer;
use crate::support::session_access_counter::SessionAccessCounter;
use crate::support::signals::install_signal_handlers;
use crate::support::stdio_child::{CommandSpec, StdioChild};
use crate::types::RuntimeArgs;

const SESSION_NOTIFICATION_BUFFER: usize = 256;
const SESSION_PENDING_REQUEST_BUFFER: usize = 512;
const SESSION_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone)]
struct AppState {
    runtime: RuntimeArgsStore,
    base_headers: HeaderMap,
    manager: Arc<SessionManager>,
    protocol_version: String,
    stdio_cmd: String,
}

#[derive(Clone)]
struct SessionResponseGuard {
    inner: Arc<SessionResponseGuardInner>,
}

struct SessionResponseGuardInner {
    manager: Arc<SessionManager>,
    session_id: String,
    reason: String,
    fired: AtomicBool,
}

impl Drop for SessionResponseGuard {
    fn drop(&mut self) {
        if self.inner.fired.swap(true, Ordering::SeqCst) {
            return;
        }
        let manager = self.inner.manager.clone();
        let session_id = self.inner.session_id.clone();
        let reason = self.inner.reason.clone();
        tokio::spawn(async move {
            manager.session_dec(&session_id, &reason).await;
        });
    }
}

fn attach_session_guard(
    response: &mut Response,
    manager: Arc<SessionManager>,
    session_id: String,
    reason: &str,
) {
    response.extensions_mut().insert(SessionResponseGuard {
        inner: Arc::new(SessionResponseGuardInner {
            manager,
            session_id,
            reason: reason.to_string(),
            fired: AtomicBool::new(false),
        }),
    });
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
    tracing::info!("  - streamableHttpPath: {}", config.streamable_http_path);
    if config.stateful {
        tracing::info!(
            "  - Session timeout: {}",
            config
                .session_timeout
                .map(|v| format!("{v}ms"))
                .unwrap_or_else(|| "disabled".to_string())
        );
    }

    let spec = parse_command_spec(&stdio_cmd)?;
    let manager = Arc::new(SessionManager::new(
        spec,
        runtime.clone(),
        config.session_timeout,
    ));

    let state = AppState {
        runtime: runtime.clone(),
        base_headers: header_map_from(&config.headers),
        manager: manager.clone(),
        protocol_version: config.protocol_version.clone(),
        stdio_cmd,
    };

    let runtime_store = runtime.clone();
    let manager_clone = manager.clone();
    let stateful = config.stateful;
    tokio::spawn(async move {
        while let Some(req) = updates.recv().await {
            let result = match req.update.scope {
                RuntimeScope::Global => {
                    let update_result = runtime_store.update_global(req.update.update).await;
                    if update_result.restart_needed {
                        if stateful {
                            manager_clone.restart_all().await;
                            RuntimeApplyResult::ok(
                                "Restarted all sessions with new runtime args",
                                true,
                            )
                        } else {
                            RuntimeApplyResult::ok(
                                "Updated runtime args for future requests",
                                false,
                            )
                        }
                    } else {
                        RuntimeApplyResult::ok("Updated runtime args", false)
                    }
                }
                RuntimeScope::Session(session_id) => {
                    if !stateful {
                        RuntimeApplyResult::error(
                            "Per-session overrides require stateful Streamable HTTP",
                        )
                    } else {
                        let update_result = runtime_store
                            .update_session(&session_id, req.update.update)
                            .await;
                        if update_result.restart_needed {
                            if manager_clone.restart_session(&session_id).await.is_err() {
                                RuntimeApplyResult::error("Failed to restart session")
                            } else {
                                RuntimeApplyResult::ok(
                                    "Restarted session with new runtime args",
                                    true,
                                )
                            }
                        } else {
                            RuntimeApplyResult::ok("Updated session runtime args", false)
                        }
                    }
                }
            };
            let _ = req.respond_to.send(result);
        }
    });

    let mut router = Router::new();

    if config.stateful {
        router = router
            .route(&config.streamable_http_path, post(stateful_post))
            .route(&config.streamable_http_path, get(stateful_get))
            .route(&config.streamable_http_path, delete(stateful_delete));
    } else {
        router = router
            .route(&config.streamable_http_path, post(stateless_post))
            .route(
                &config.streamable_http_path,
                get(stateless_method_not_allowed),
            )
            .route(
                &config.streamable_http_path,
                delete(stateless_method_not_allowed),
            );
    }

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

    let router = router.with_state(state.clone());

    install_signal_handlers(None);

    let addr: std::net::SocketAddr = ([0, 0, 0, 0], config.port).into();
    tracing::info!("Listening on port {}", config.port);
    tracing::info!(
        "StreamableHttp endpoint: http://localhost:{}{}",
        config.port,
        config.streamable_http_path
    );

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|err| err.to_string())?;
    axum::serve(listener, router.into_make_service())
        .await
        .map_err(|err| err.to_string())
}

async fn health_handler(state: AppState) -> impl IntoResponse {
    let mut response = (StatusCode::OK, "ok").into_response();
    apply_headers(&state, None, &mut response).await;
    response
}

async fn stateless_post(
    State(state): State<AppState>,
    Json(payload): Json<serde_json::Value>,
) -> impl IntoResponse {
    let runtime = state.runtime.get_effective(None).await;
    match handle_stateless_request(&state.stdio_cmd, &state.protocol_version, runtime, payload)
        .await
    {
        Ok(Some(resp)) => {
            let mut response = Json(resp).into_response();
            apply_headers(&state, None, &mut response).await;
            response
        }
        Ok(None) => {
            let mut response = StatusCode::NO_CONTENT.into_response();
            apply_headers(&state, None, &mut response).await;
            response
        }
        Err(err) => {
            let mut response = (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "error": { "code": -32603, "message": err },
                    "id": serde_json::Value::Null
                })),
            )
                .into_response();
            apply_headers(&state, None, &mut response).await;
            response
        }
    }
}

async fn stateless_method_not_allowed() -> impl IntoResponse {
    (
        StatusCode::METHOD_NOT_ALLOWED,
        Json(serde_json::json!({
            "jsonrpc": "2.0",
            "error": { "code": -32000, "message": "Method not allowed." },
            "id": serde_json::Value::Null
        })),
    )
}

async fn stateful_post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<serde_json::Value>,
) -> impl IntoResponse {
    let session_header = headers
        .get("Mcp-Session-Id")
        .or_else(|| headers.get("mcp-session-id"))
        .and_then(|v| v.to_str().ok())
        .map(|v| v.to_string());

    let mut should_inc = false;
    let (session_id, session) = if let Some(id) = session_header {
        if let Some(session) = state.manager.get_session(&id).await {
            should_inc = true;
            (id, session)
        } else {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "error": { "code": -32000, "message": "Bad Request: No valid session ID provided" },
                    "id": serde_json::Value::Null
                })),
            )
                .into_response();
        }
    } else if is_initialize_request(&payload) {
        let session = match state.manager.create_session().await {
            Ok(session) => session,
            Err(err) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({
                        "jsonrpc": "2.0",
                        "error": { "code": -32603, "message": err },
                        "id": serde_json::Value::Null
                    })),
                )
                    .into_response();
            }
        };
        let session_id = session.id.clone();
        (session_id, session)
    } else {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "jsonrpc": "2.0",
                "error": { "code": -32000, "message": "Bad Request: No valid session ID provided" },
                "id": serde_json::Value::Null
            })),
        )
            .into_response();
    };

    if should_inc {
        state
            .manager
            .session_inc(&session_id, "POST request for existing session")
            .await;
    }

    let response = if let Some(id) = payload.get("id").cloned() {
        match session.request(payload).await {
            Ok(resp) => Json(resp).into_response(),
            Err(err) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "error": { "code": -32603, "message": err },
                    "id": id
                })),
            )
                .into_response(),
        }
    } else if session.send(&payload).await.is_err() {
        (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({
                "jsonrpc": "2.0",
                "error": { "code": -32603, "message": "Failed to send message" },
                "id": serde_json::Value::Null
            })),
        )
            .into_response()
    } else {
        StatusCode::NO_CONTENT.into_response()
    };

    let mut response = response;
    response.headers_mut().insert(
        "Mcp-Session-Id",
        HeaderValue::from_str(&session_id).unwrap(),
    );
    apply_headers(&state, Some(&session_id), &mut response).await;
    attach_session_guard(
        &mut response,
        state.manager.clone(),
        session_id,
        "POST response closed",
    );
    response
}

async fn stateful_get(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    let session_id = headers
        .get("Mcp-Session-Id")
        .or_else(|| headers.get("mcp-session-id"))
        .and_then(|v| v.to_str().ok())
        .map(|v| v.to_string());

    let Some(session_id) = session_id else {
        return (StatusCode::BAD_REQUEST, "Invalid or missing session ID").into_response();
    };
    let Some(session) = state.manager.get_session(&session_id).await else {
        return (StatusCode::BAD_REQUEST, "Invalid or missing session ID").into_response();
    };

    state
        .manager
        .session_inc(&session_id, "GET request for existing session")
        .await;

    let rx = session.notifications.subscribe();
    let stream = BroadcastStream::new(rx).filter_map(|msg| async move {
        match msg {
            Ok(value) => {
                let data = serde_json::to_string(&value).ok()?;
                Some(Ok::<Event, std::convert::Infallible>(
                    Event::default().data(data),
                ))
            }
            Err(_) => None,
        }
    });
    let sse = Sse::new(stream).keep_alive(axum::response::sse::KeepAlive::default());
    let mut response = sse.into_response();
    response.headers_mut().insert(
        "Mcp-Session-Id",
        HeaderValue::from_str(&session_id).unwrap(),
    );
    apply_headers(&state, Some(&session_id), &mut response).await;
    attach_session_guard(
        &mut response,
        state.manager.clone(),
        session_id,
        "GET response closed",
    );
    response
}

async fn stateful_delete(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    let session_id = headers
        .get("Mcp-Session-Id")
        .or_else(|| headers.get("mcp-session-id"))
        .and_then(|v| v.to_str().ok())
        .map(|v| v.to_string());

    let Some(session_id) = session_id else {
        return (StatusCode::BAD_REQUEST, "Invalid or missing session ID").into_response();
    };

    if state.manager.get_session(&session_id).await.is_none() {
        return (StatusCode::BAD_REQUEST, "Invalid or missing session ID").into_response();
    }

    state
        .manager
        .session_inc(&session_id, "DELETE request for existing session")
        .await;

    if state.manager.remove_session(&session_id).await {
        let mut response = StatusCode::OK.into_response();
        apply_headers(&state, Some(&session_id), &mut response).await;
        attach_session_guard(
            &mut response,
            state.manager.clone(),
            session_id,
            "DELETE response closed",
        );
        response
    } else {
        (StatusCode::BAD_REQUEST, "Invalid or missing session ID").into_response()
    }
}

async fn apply_headers(state: &AppState, session_id: Option<&str>, response: &mut Response) {
    let runtime = state.runtime.get_effective(session_id).await;
    let headers = merge_headers(&state.base_headers, &runtime.headers);
    let header_map = response.headers_mut();
    for (key, value) in headers.iter() {
        header_map.insert(key, value.clone());
    }
}

struct Session {
    id: String,
    child: Arc<StdioChild>,
    pending: Arc<Mutex<HashMap<String, oneshot::Sender<serde_json::Value>>>>,
    notifications: broadcast::Sender<serde_json::Value>,
}

impl Session {
    async fn new(id: String, spec: CommandSpec, runtime: RuntimeArgs) -> Result<Self, String> {
        let child = Arc::new(StdioChild::new(spec, false));
        child.spawn(&runtime).await?;
        let (tx, _) = broadcast::channel(SESSION_NOTIFICATION_BUFFER);
        Ok(Session {
            id,
            child,
            pending: Arc::new(Mutex::new(HashMap::new())),
            notifications: tx,
        })
    }

    async fn start_routing(self: Arc<Self>) {
        let mut rx = self.child.subscribe();
        let this = self.clone();
        tokio::spawn(async move {
            while let Ok(msg) = rx.recv().await {
                if let Some(id) = msg.get("id").and_then(|v| v.as_str()) {
                    let sender = {
                        let mut pending = this.pending.lock().await;
                        pending.remove(id)
                    };
                    if let Some(sender) = sender {
                        let _ = sender.send(msg);
                        continue;
                    }
                }
                let _ = this.notifications.send(msg);
            }
        });
    }

    async fn send(&self, message: &serde_json::Value) -> Result<(), String> {
        self.child.send(message).await
    }

    async fn request(&self, message: serde_json::Value) -> Result<serde_json::Value, String> {
        let id = message
            .get("id")
            .and_then(|v| v.as_str())
            .map(|v| v.to_string())
            .unwrap_or_else(|| message.get("id").unwrap().to_string());
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending.lock().await;
            if pending.contains_key(&id) {
                return Err(format!("Request id already in-flight: {id}"));
            }
            if pending.len() >= SESSION_PENDING_REQUEST_BUFFER {
                return Err(format!(
                    "Too many in-flight requests in session (max {})",
                    SESSION_PENDING_REQUEST_BUFFER
                ));
            }
            pending.insert(id.clone(), tx);
        }

        let pending_for_timeout = self.pending.clone();
        let id_for_timeout = id.clone();
        let cleanup_task = tokio::spawn(async move {
            tokio::time::sleep(SESSION_REQUEST_TIMEOUT).await;
            let mut pending = pending_for_timeout.lock().await;
            pending.remove(&id_for_timeout);
        });

        if let Err(err) = self.child.send(&message).await {
            cleanup_task.abort();
            let mut pending = self.pending.lock().await;
            pending.remove(&id);
            return Err(err);
        }

        let result = match tokio::time::timeout(SESSION_REQUEST_TIMEOUT, rx).await {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(_)) => {
                let mut pending = self.pending.lock().await;
                pending.remove(&id);
                Err("Request cancelled".to_string())
            }
            Err(_) => {
                let mut pending = self.pending.lock().await;
                pending.remove(&id);
                Err(format!(
                    "Request timed out after {}ms",
                    SESSION_REQUEST_TIMEOUT.as_millis()
                ))
            }
        };

        cleanup_task.abort();
        result
    }

    async fn restart(&self, runtime: &RuntimeArgs) -> Result<(), String> {
        self.child.restart(runtime).await
    }
}

struct SessionManager {
    spec: CommandSpec,
    runtime: RuntimeArgsStore,
    sessions: Arc<Mutex<HashMap<String, Arc<Session>>>>,
    session_counter: Option<Arc<SessionAccessCounter>>,
}

impl SessionManager {
    fn new(spec: CommandSpec, runtime: RuntimeArgsStore, session_timeout: Option<u64>) -> Self {
        let sessions: Arc<Mutex<HashMap<String, Arc<Session>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let session_counter = session_timeout.map(|timeout| {
            let sessions_clone = sessions.clone();
            Arc::new(SessionAccessCounter::new(
                timeout,
                Arc::new(move |session_id| {
                    tracing::info!("Session {session_id} timed out, cleaning up");
                    let sessions_inner = sessions_clone.clone();
                    tokio::spawn(async move {
                        if let Some(session) = sessions_inner.lock().await.remove(&session_id) {
                            session.child.shutdown().await;
                        }
                    });
                }),
            ))
        });
        Self {
            spec,
            runtime,
            sessions,
            session_counter,
        }
    }

    async fn create_session(&self) -> Result<Arc<Session>, String> {
        let session_id = Uuid::new_v4().to_string();
        let runtime = self.runtime.get_effective(Some(&session_id)).await;
        let session = Arc::new(Session::new(session_id.clone(), self.spec.clone(), runtime).await?);
        session.clone().start_routing().await;
        let mut sessions = self.sessions.lock().await;
        sessions.insert(session_id.clone(), session.clone());
        if let Some(counter) = &self.session_counter {
            counter.inc(&session_id, "session initialization").await;
        }
        Ok(session)
    }

    async fn get_session(&self, session_id: &str) -> Option<Arc<Session>> {
        let sessions = self.sessions.lock().await;
        sessions.get(session_id).cloned()
    }

    async fn remove_session(&self, session_id: &str) -> bool {
        let mut sessions = self.sessions.lock().await;
        let removed = sessions.remove(session_id);
        if let Some(session) = removed {
            session.child.shutdown().await;
            true
        } else {
            false
        }
    }

    async fn session_inc(&self, session_id: &str, reason: &str) {
        if let Some(counter) = &self.session_counter {
            counter.inc(session_id, reason).await;
        }
    }

    async fn session_dec(&self, session_id: &str, reason: &str) {
        if let Some(counter) = &self.session_counter {
            counter.dec(session_id, reason).await;
        }
    }

    async fn restart_session(&self, session_id: &str) -> Result<(), String> {
        let runtime = self.runtime.get_effective(Some(session_id)).await;
        let sessions = self.sessions.lock().await;
        if let Some(session) = sessions.get(session_id) {
            session.restart(&runtime).await?;
        }
        Ok(())
    }

    async fn restart_all(&self) {
        let ids: Vec<String> = {
            let sessions = self.sessions.lock().await;
            sessions.keys().cloned().collect()
        };
        for id in ids {
            let _ = self.restart_session(&id).await;
        }
    }
}

async fn handle_stateless_request(
    stdio_cmd: &str,
    protocol_version: &str,
    runtime: RuntimeArgs,
    payload: serde_json::Value,
) -> Result<Option<serde_json::Value>, String> {
    use tokio::io::AsyncWriteExt;
    use tokio_util::codec::{FramedRead, LinesCodec};

    let spec = parse_command_spec(stdio_cmd)?;
    let mut cmd = spec.build_command(&runtime);
    cmd.stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = cmd.spawn().map_err(|err| err.to_string())?;
    let mut stdin = child.stdin.take().ok_or("Missing child stdin")?;
    let stdout = child.stdout.take().ok_or("Missing child stdout")?;
    let stderr = child.stderr.take().ok_or("Missing child stderr")?;

    tokio::spawn(async move {
        let mut lines = FramedRead::new(stderr, LinesCodec::new());
        while let Some(Ok(line)) = lines.next().await {
            if !line.trim().is_empty() {
                eprintln!("[mcpway] Child stderr: {line}");
            }
        }
    });

    let original_id = payload.get("id").map(|v| v.to_string());
    if original_id.is_none() {
        let line = serde_json::to_string(&payload).map_err(|err| err.to_string())?;
        stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|err| err.to_string())?;
        stdin
            .write_all(b"\n")
            .await
            .map_err(|err| err.to_string())?;
        let _ = child.kill().await;
        return Ok(None);
    }

    let is_init = is_initialize_request(&payload);
    let mut pending_original: Option<serde_json::Value> = None;
    let mut auto_init_id: Option<String> = None;

    if !is_init {
        let init_id = format!(
            "init_{}_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis(),
            Uuid::new_v4()
        );
        auto_init_id = Some(init_id.clone());
        let init = create_initialize_request(&init_id, protocol_version);
        let init_line = serde_json::to_string(&init).map_err(|err| err.to_string())?;
        stdin
            .write_all(init_line.as_bytes())
            .await
            .map_err(|err| err.to_string())?;
        stdin
            .write_all(b"\n")
            .await
            .map_err(|err| err.to_string())?;
        pending_original = Some(payload.clone());
    } else {
        let line = serde_json::to_string(&payload).map_err(|err| err.to_string())?;
        stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|err| err.to_string())?;
        stdin
            .write_all(b"\n")
            .await
            .map_err(|err| err.to_string())?;
    }

    let mut lines = FramedRead::new(stdout, LinesCodec::new());
    while let Some(line) = lines.next().await {
        match line {
            Ok(line) => {
                if line.trim().is_empty() {
                    continue;
                }
                let msg: serde_json::Value = match serde_json::from_str(&line) {
                    Ok(val) => val,
                    Err(_) => continue,
                };
                if let Some(id) = msg.get("id").map(|v| v.to_string()) {
                    if let Some(auto_id) = auto_init_id.clone() {
                        if id.trim_matches('"') == auto_id {
                            let initialized = create_initialized_notification();
                            let init_line = serde_json::to_string(&initialized)
                                .map_err(|err| err.to_string())?;
                            stdin
                                .write_all(init_line.as_bytes())
                                .await
                                .map_err(|err| err.to_string())?;
                            stdin
                                .write_all(b"\n")
                                .await
                                .map_err(|err| err.to_string())?;
                            if let Some(original) = pending_original.take() {
                                let line = serde_json::to_string(&original)
                                    .map_err(|err| err.to_string())?;
                                stdin
                                    .write_all(line.as_bytes())
                                    .await
                                    .map_err(|err| err.to_string())?;
                                stdin
                                    .write_all(b"\n")
                                    .await
                                    .map_err(|err| err.to_string())?;
                            }
                            auto_init_id = None;
                            continue;
                        }
                    }
                    if let Some(target_id) = original_id.clone() {
                        if id == target_id {
                            let _ = child.kill().await;
                            return Ok(Some(msg));
                        }
                    }
                }
            }
            Err(_) => break,
        }
    }
    let _ = child.kill().await;
    Err("Child terminated before response".to_string())
}

fn is_initialize_request(msg: &serde_json::Value) -> bool {
    msg.get("method")
        .and_then(|m| m.as_str())
        .map(|m| m == "initialize")
        .unwrap_or(false)
}

fn create_initialize_request(id: &str, protocol_version: &str) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "initialize",
        "params": {
            "protocolVersion": protocol_version,
            "capabilities": {
                "roots": { "listChanged": true },
                "sampling": {}
            },
            "clientInfo": {
                "name": "mcpway",
                "version": crate::support::version::get_version()
            }
        }
    })
}

fn create_initialized_notification() -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized"
    })
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
