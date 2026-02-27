use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, Request, State};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures::{SinkExt, StreamExt};
use include_dir::{include_dir, Dir};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{broadcast, Mutex};

use crate::config::{OutputTransport, WebConfig};
use crate::discovery::user_home_dir;
use crate::support::browser_launch::{should_attempt_browser_launch, try_open_browser};
use crate::support::log_store::{default_log_path, ensure_log_file, parse_record, StoredLogRecord};
use crate::support::telemetry::init_telemetry;
use mcpway::tool_api::{ToolCallError, ToolClient, ToolClientBuilder, ToolMetadata, Transport};

static WEB_DIST: Dir<'_> = include_dir!("$OUT_DIR/web-dist");
const LOG_STREAM_BUFFER: usize = 2048;
const INSPECT_MAX_SESSIONS: usize = 64;
const INSPECT_MAX_HISTORY_PER_SESSION: usize = 400;
const INSPECT_DEFAULT_PROTOCOL_VERSION: &str = "2024-11-05";

#[derive(Clone)]
struct AppState {
    log_path: PathBuf,
    log_sender: broadcast::Sender<StoredLogRecord>,
    auth_token: Option<String>,
    admin_proxy: Option<AdminProxy>,
    themes: ThemeService,
    inspect: InspectManager,
    hot_reload_url: Option<String>,
}

#[derive(Clone)]
struct AdminProxy {
    base_url: String,
    token: Option<String>,
    client: reqwest::Client,
}

impl AdminProxy {
    fn new(base_url: String, token: Option<String>) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            token,
            client: reqwest::Client::new(),
        }
    }

    async fn get_json(&self, path: &str) -> Result<Value, String> {
        let url = format!("{}{}", self.base_url, path);
        let mut request = self.client.get(url);
        if let Some(token) = self.token.as_deref() {
            request = request.bearer_auth(token);
        }

        let response = request
            .send()
            .await
            .map_err(|err| format!("Admin request failed: {err}"))?;
        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|err| format!("Failed to read admin response: {err}"))?;

        if !status.is_success() {
            return Err(format!(
                "Admin request returned HTTP {} with body: {}",
                status.as_u16(),
                body
            ));
        }

        serde_json::from_str(&body)
            .map_err(|err| format!("Admin response is not valid JSON: {err}; body: {body}"))
    }

    async fn post_json(&self, path: &str, payload: &Value) -> Result<Value, String> {
        let url = format!("{}{}", self.base_url, path);
        let mut request = self.client.post(url).json(payload);
        if let Some(token) = self.token.as_deref() {
            request = request.bearer_auth(token);
        }

        let response = request
            .send()
            .await
            .map_err(|err| format!("Admin request failed: {err}"))?;
        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|err| format!("Failed to read admin response: {err}"))?;

        if !status.is_success() {
            return Err(format!(
                "Admin request returned HTTP {} with body: {}",
                status.as_u16(),
                body
            ));
        }

        serde_json::from_str(&body)
            .map_err(|err| format!("Admin response is not valid JSON: {err}; body: {body}"))
    }
}

#[derive(Clone)]
struct ThemeService {
    catalog_url: String,
    cache_file: PathBuf,
    cache_ttl_seconds: u64,
    client: reqwest::Client,
    lock: Arc<Mutex<()>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ThemePalette {
    background: String,
    foreground: String,
    cursor: String,
    ansi: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ThemeDescriptor {
    id: String,
    name: String,
    source_url: String,
    palette: ThemePalette,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ThemeCatalog {
    fetched_at_utc: u64,
    themes: Vec<ThemeDescriptor>,
}

#[derive(Debug, Deserialize)]
struct LogsRecentQuery {
    lines: Option<usize>,
    level: Option<String>,
    transport: Option<String>,
    search: Option<String>,
}

#[derive(Clone)]
struct InspectManager {
    sessions: Arc<Mutex<HashMap<String, Arc<InspectSession>>>>,
    max_sessions: usize,
    max_history_per_session: usize,
    session_seq: Arc<AtomicU64>,
}

struct InspectSession {
    id: String,
    name: String,
    transport: InspectTransportKind,
    endpoint: String,
    created_at_utc: u64,
    state: Arc<Mutex<InspectSessionState>>,
    history: Arc<Mutex<VecDeque<InspectHistoryEntry>>>,
    notifications: Arc<Mutex<VecDeque<InspectNotificationEntry>>>,
    history_seq: AtomicU64,
    notification_seq: AtomicU64,
    max_history_entries: usize,
    client: ToolClient,
}

#[derive(Debug, Clone)]
struct InspectSessionState {
    status: String,
    last_error: Option<String>,
    updated_at_utc: u64,
    metadata: HashMap<String, String>,
    auth_header_name: Option<String>,
    auth_bearer_token: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct InspectSessionSummary {
    session_id: String,
    session_name: String,
    transport: String,
    endpoint: String,
    status: String,
    last_error: Option<String>,
    connected_at_utc: u64,
    updated_at_utc: u64,
    history_size: usize,
    notifications_size: usize,
}

#[derive(Debug, Clone, Serialize)]
struct InspectHistoryEntry {
    id: String,
    ts_utc: u64,
    kind: String,
    summary: String,
    request: Option<Value>,
    response: Option<Value>,
    error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct InspectNotificationEntry {
    id: String,
    ts_utc: u64,
    method: String,
    payload: Value,
    summary: String,
}

#[derive(Debug, Clone, Serialize)]
struct InspectToolDescriptor {
    name: String,
    description: Option<String>,
    input_schema: Value,
}

#[derive(Debug, Clone, Serialize)]
struct InspectToolsListResponse {
    session_id: String,
    tool_count: usize,
    duration_ms: u128,
    tools: Vec<InspectToolDescriptor>,
}

#[derive(Debug, Clone, Serialize)]
struct InspectToolCallResponse {
    session_id: String,
    tool_name: String,
    started_at_utc: u64,
    duration_ms: u128,
    result: Value,
}

#[derive(Debug, Deserialize)]
struct InspectConnectRequest {
    transport: String,
    endpoint: Option<String>,
    command: Option<String>,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: HashMap<String, String>,
    cwd: Option<String>,
    #[serde(default)]
    headers: HashMap<String, String>,
    protocol_version: Option<String>,
    session_name: Option<String>,
    connect_timeout_ms: Option<u64>,
    request_timeout_ms: Option<u64>,
}

#[derive(Debug, Serialize)]
struct InspectConnectResponse {
    session: InspectSessionSummary,
    tools_count: usize,
}

#[derive(Debug, Deserialize)]
struct InspectDisconnectRequest {
    session_id: String,
}

#[derive(Debug, Deserialize)]
struct InspectToolsListRequest {
    session_id: String,
}

#[derive(Debug, Deserialize)]
struct InspectToolCallRequest {
    session_id: String,
    tool_name: String,
    #[serde(default = "empty_json_object")]
    arguments: Value,
}

#[derive(Debug, Deserialize)]
struct InspectSessionQuery {
    session_id: String,
}

#[derive(Debug, Deserialize)]
struct InspectHistoryQuery {
    session_id: String,
    limit: Option<usize>,
}

#[derive(Debug, Serialize)]
struct InspectHistoryResponse {
    session_id: String,
    count: usize,
    entries: Vec<InspectHistoryEntry>,
}

#[derive(Debug, Serialize)]
struct InspectNotificationsResponse {
    session_id: String,
    count: usize,
    entries: Vec<InspectNotificationEntry>,
}

#[derive(Debug, Serialize)]
struct InspectDisconnectResponse {
    session_id: String,
    disconnected: bool,
}

#[derive(Debug, Serialize)]
struct InspectHistoryClearResponse {
    session_id: String,
    cleared: usize,
}

#[derive(Debug, Deserialize)]
struct InspectNotificationsQuery {
    session_id: String,
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct InspectPromptsListRequest {
    session_id: String,
    cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
struct InspectPromptGetRequest {
    session_id: String,
    name: String,
    #[serde(default = "empty_json_object")]
    arguments: Value,
}

#[derive(Debug, Deserialize)]
struct InspectResourcesListRequest {
    session_id: String,
    cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
struct InspectResourceTemplatesListRequest {
    session_id: String,
    cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
struct InspectResourceReadRequest {
    session_id: String,
    uri: String,
}

#[derive(Debug, Deserialize)]
struct InspectResourceSubscriptionRequest {
    session_id: String,
    uri: String,
}

#[derive(Debug, Deserialize)]
struct InspectRootsSetRequest {
    session_id: String,
    #[serde(default)]
    roots: Vec<Value>,
}

#[derive(Debug, Deserialize)]
struct InspectTaskByIdRequest {
    session_id: String,
    task_id: String,
}

#[derive(Debug, Deserialize)]
struct InspectPingRequest {
    session_id: String,
}

#[derive(Debug, Deserialize)]
struct InspectMetadataSetRequest {
    session_id: String,
    #[serde(default)]
    metadata: HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
struct InspectAuthSetRequest {
    session_id: String,
    header_name: Option<String>,
    bearer_token: Option<String>,
}

#[derive(Debug, Serialize)]
struct InspectRpcResultResponse {
    session_id: String,
    method: String,
    duration_ms: u128,
    result: Value,
}

#[derive(Debug, Serialize)]
struct InspectMetadataResponse {
    session_id: String,
    metadata: HashMap<String, String>,
}

#[derive(Debug, Serialize)]
struct InspectAuthStateResponse {
    session_id: String,
    header_name: Option<String>,
    has_bearer_token: bool,
}

#[derive(Debug, Clone, Copy)]
enum InspectTransportKind {
    Stdio,
    StreamableHttp,
    Sse,
    Ws,
    Grpc,
}

impl InspectTransportKind {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Stdio => "stdio",
            Self::StreamableHttp => "streamable-http",
            Self::Sse => "sse",
            Self::Ws => "ws",
            Self::Grpc => "grpc",
        }
    }

    fn to_tool_transport(self) -> Option<Transport> {
        match self {
            Self::Stdio => None,
            Self::StreamableHttp => Some(Transport::StreamableHttp),
            Self::Sse => Some(Transport::Sse),
            Self::Ws => Some(Transport::Ws),
            Self::Grpc => Some(Transport::Grpc),
        }
    }
}

impl InspectManager {
    fn new(max_sessions: usize, max_history_per_session: usize) -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            max_sessions: max_sessions.max(1),
            max_history_per_session: max_history_per_session.max(1),
            session_seq: Arc::new(AtomicU64::new(0)),
        }
    }

    async fn connect(
        &self,
        request: InspectConnectRequest,
    ) -> Result<InspectConnectResponse, (StatusCode, String)> {
        let transport = parse_inspect_transport(request.transport.as_str())
            .map_err(|message| (StatusCode::BAD_REQUEST, message))?;
        if matches!(transport, InspectTransportKind::Stdio) {
            let command = request
                .command
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty());
            let args_count = request.args.len();
            let env_count = request.env.len();
            let cwd = request.cwd.as_deref().unwrap_or("");
            return Err((
                StatusCode::NOT_IMPLEMENTED,
                format!(
                    "stdio transport is not available yet for web inspector (command='{}', args={args_count}, env={env_count}, cwd='{}'). Use streamable-http, sse, ws, or grpc.",
                    command.unwrap_or(""),
                    cwd
                ),
            ));
        }

        let endpoint = request
            .endpoint
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .ok_or_else(|| {
                (
                    StatusCode::BAD_REQUEST,
                    "endpoint cannot be empty for non-stdio transports".to_string(),
                )
            })?;

        {
            let sessions = self.sessions.lock().await;
            if sessions.len() >= self.max_sessions {
                return Err((
                    StatusCode::CONFLICT,
                    format!(
                        "inspect session limit reached ({}). Disconnect a session before connecting a new one.",
                        self.max_sessions
                    ),
                ));
            }
        }

        let connect_timeout_ms = request
            .connect_timeout_ms
            .unwrap_or(10_000)
            .clamp(200, 120_000);
        let request_timeout = request.request_timeout_ms.map(|ms| {
            if ms == 0 {
                None
            } else {
                Some(Duration::from_millis(ms.clamp(200, 300_000)))
            }
        });
        let protocol_version = request
            .protocol_version
            .as_deref()
            .unwrap_or(INSPECT_DEFAULT_PROTOCOL_VERSION)
            .trim();
        if protocol_version.is_empty() {
            return Err((
                StatusCode::BAD_REQUEST,
                "protocol_version cannot be empty when provided".to_string(),
            ));
        }

        let mut headers = HashMap::new();
        for (key, value) in request.headers {
            let key = key.trim();
            if key.is_empty() {
                continue;
            }
            headers.insert(key.to_string(), value);
        }

        let transport_kind = transport.to_tool_transport().ok_or_else(|| {
            (
                StatusCode::NOT_IMPLEMENTED,
                "unsupported inspect transport for tool client".to_string(),
            )
        })?;

        let mut builder = ToolClientBuilder::new(endpoint.clone(), transport_kind)
            .protocol_version(protocol_version.to_string())
            .headers(headers)
            .connect_timeout(Duration::from_millis(connect_timeout_ms));
        if let Some(timeout) = request_timeout {
            builder = builder.request_timeout(timeout);
        }
        let client = builder.build().map_err(|err| {
            (
                StatusCode::BAD_REQUEST,
                format!("inspect connect failed: {err}"),
            )
        })?;

        let tools_count = match client.refresh_tools().await {
            Ok(()) => client.tools().list().await.len(),
            Err(err) => {
                return Err((
                    status_for_tool_error(&err),
                    format!("inspect handshake failed: {err}"),
                ))
            }
        };

        let session_id = self.next_session_id();
        let session_name = request
            .session_name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| default_session_name(transport, endpoint.as_str()));
        let session = Arc::new(InspectSession::new(
            session_id.clone(),
            session_name,
            transport,
            endpoint,
            self.max_history_per_session,
            client,
        ));
        session
            .push_history("session.connect", "Session connected", None, None, None)
            .await;

        self.insert_session(session.clone()).await?;
        let summary = session.summary().await;

        Ok(InspectConnectResponse {
            session: summary,
            tools_count,
        })
    }

    async fn insert_session(
        &self,
        session: Arc<InspectSession>,
    ) -> Result<(), (StatusCode, String)> {
        let mut sessions = self.sessions.lock().await;
        if sessions.len() >= self.max_sessions {
            return Err((
                StatusCode::CONFLICT,
                format!(
                    "inspect session limit reached ({}). Disconnect a session before connecting a new one.",
                    self.max_sessions
                ),
            ));
        }
        sessions.insert(session.id.clone(), session);
        Ok(())
    }

    async fn session_count(&self) -> usize {
        self.sessions.lock().await.len()
    }

    async fn get_session(&self, session_id: &str) -> Option<Arc<InspectSession>> {
        self.sessions.lock().await.get(session_id).cloned()
    }

    async fn remove_session(&self, session_id: &str) -> Option<Arc<InspectSession>> {
        self.sessions.lock().await.remove(session_id)
    }

    async fn list_sessions(&self) -> Vec<InspectSessionSummary> {
        let sessions = {
            let sessions = self.sessions.lock().await;
            sessions.values().cloned().collect::<Vec<_>>()
        };

        let mut summaries = Vec::with_capacity(sessions.len());
        for session in sessions {
            summaries.push(session.summary().await);
        }
        summaries.sort_by(|left, right| right.connected_at_utc.cmp(&left.connected_at_utc));
        summaries
    }

    fn next_session_id(&self) -> String {
        let seq = self.session_seq.fetch_add(1, Ordering::Relaxed) + 1;
        format!("inspect-{}-{seq}", unix_timestamp_secs())
    }
}

impl InspectSession {
    fn new(
        id: String,
        name: String,
        transport: InspectTransportKind,
        endpoint: String,
        max_history_entries: usize,
        client: ToolClient,
    ) -> Self {
        let now = unix_timestamp_secs();
        Self {
            id,
            name,
            transport,
            endpoint,
            created_at_utc: now,
            state: Arc::new(Mutex::new(InspectSessionState {
                status: "connected".to_string(),
                last_error: None,
                updated_at_utc: now,
                metadata: HashMap::new(),
                auth_header_name: None,
                auth_bearer_token: None,
            })),
            history: Arc::new(Mutex::new(VecDeque::with_capacity(
                max_history_entries.max(1),
            ))),
            notifications: Arc::new(Mutex::new(VecDeque::with_capacity(
                max_history_entries.max(1),
            ))),
            history_seq: AtomicU64::new(0),
            notification_seq: AtomicU64::new(0),
            max_history_entries: max_history_entries.max(1),
            client,
        }
    }

    async fn summary(&self) -> InspectSessionSummary {
        let state = self.state.lock().await.clone();
        let history_size = self.history.lock().await.len();
        let notifications_size = self.notifications.lock().await.len();
        InspectSessionSummary {
            session_id: self.id.clone(),
            session_name: self.name.clone(),
            transport: self.transport.as_str().to_string(),
            endpoint: self.endpoint.clone(),
            status: state.status,
            last_error: state.last_error,
            connected_at_utc: self.created_at_utc,
            updated_at_utc: state.updated_at_utc,
            history_size,
            notifications_size,
        }
    }

    async fn list_tools(&self) -> Result<InspectToolsListResponse, ToolCallError> {
        let started = Instant::now();
        self.client.refresh_tools().await?;
        let tools = self.client.tools().list().await;
        let tool_descriptors = tools
            .into_iter()
            .map(map_tool_descriptor)
            .collect::<Vec<_>>();
        Ok(InspectToolsListResponse {
            session_id: self.id.clone(),
            tool_count: tool_descriptors.len(),
            duration_ms: started.elapsed().as_millis(),
            tools: tool_descriptors,
        })
    }

    async fn call_tool(
        &self,
        tool_name: &str,
        arguments: Value,
    ) -> Result<InspectToolCallResponse, ToolCallError> {
        let started_at_utc = unix_timestamp_secs();
        let started = Instant::now();
        let result = self.client.call_by_name(tool_name, arguments).await?;
        Ok(InspectToolCallResponse {
            session_id: self.id.clone(),
            tool_name: tool_name.to_string(),
            started_at_utc,
            duration_ms: started.elapsed().as_millis(),
            result,
        })
    }

    async fn request_method(
        &self,
        method: &str,
        params: Value,
    ) -> Result<InspectRpcResultResponse, ToolCallError> {
        let started = Instant::now();
        let result = self.client.request(method, params).await?;
        Ok(InspectRpcResultResponse {
            session_id: self.id.clone(),
            method: method.to_string(),
            duration_ms: started.elapsed().as_millis(),
            result,
        })
    }

    async fn notify_method(&self, method: &str, params: Value) -> Result<(), ToolCallError> {
        self.client.notify(method, params).await
    }

    async fn history(&self, limit: Option<usize>) -> Vec<InspectHistoryEntry> {
        let history = self.history.lock().await;
        let take = limit
            .unwrap_or(history.len())
            .min(self.max_history_entries)
            .min(history.len());
        let skip = history.len().saturating_sub(take);
        history.iter().skip(skip).cloned().collect()
    }

    async fn clear_history(&self) -> usize {
        let mut history = self.history.lock().await;
        let cleared = history.len();
        history.clear();
        cleared
    }

    async fn mark_connected(&self) {
        let mut state = self.state.lock().await;
        state.status = "connected".to_string();
        state.last_error = None;
        state.updated_at_utc = unix_timestamp_secs();
    }

    async fn mark_error(&self, err: String) {
        let mut state = self.state.lock().await;
        state.status = "error".to_string();
        state.last_error = Some(err);
        state.updated_at_utc = unix_timestamp_secs();
    }

    async fn mark_disconnected(&self) {
        let mut state = self.state.lock().await;
        state.status = "disconnected".to_string();
        state.updated_at_utc = unix_timestamp_secs();
    }

    async fn push_history(
        &self,
        kind: &str,
        summary: impl Into<String>,
        request: Option<Value>,
        response: Option<Value>,
        error: Option<String>,
    ) {
        let sequence = self.history_seq.fetch_add(1, Ordering::Relaxed) + 1;
        let entry = InspectHistoryEntry {
            id: format!("{}-{sequence}", self.id),
            ts_utc: unix_timestamp_secs(),
            kind: kind.to_string(),
            summary: summary.into(),
            request,
            response,
            error,
        };

        let mut history = self.history.lock().await;
        if history.len() >= self.max_history_entries {
            history.pop_front();
        }
        history.push_back(entry);
    }

    async fn notifications(&self, limit: Option<usize>) -> Vec<InspectNotificationEntry> {
        let notifications = self.notifications.lock().await;
        let take = limit
            .unwrap_or(notifications.len())
            .min(self.max_history_entries)
            .min(notifications.len());
        let skip = notifications.len().saturating_sub(take);
        notifications.iter().skip(skip).cloned().collect()
    }

    async fn clear_notifications(&self) -> usize {
        let mut notifications = self.notifications.lock().await;
        let cleared = notifications.len();
        notifications.clear();
        cleared
    }

    async fn push_notification(&self, method: &str, payload: Value, summary: impl Into<String>) {
        let sequence = self.notification_seq.fetch_add(1, Ordering::Relaxed) + 1;
        let entry = InspectNotificationEntry {
            id: format!("notify-{}-{sequence}", self.id),
            ts_utc: unix_timestamp_secs(),
            method: method.to_string(),
            payload,
            summary: summary.into(),
        };

        let mut notifications = self.notifications.lock().await;
        if notifications.len() >= self.max_history_entries {
            notifications.pop_front();
        }
        notifications.push_back(entry);
    }

    async fn set_metadata(&self, metadata: HashMap<String, String>) {
        let mut state = self.state.lock().await;
        state.metadata = metadata;
        state.updated_at_utc = unix_timestamp_secs();
    }

    async fn metadata(&self) -> HashMap<String, String> {
        self.state.lock().await.metadata.clone()
    }

    async fn set_auth_state(&self, header_name: Option<String>, bearer_token: Option<String>) {
        let mut state = self.state.lock().await;
        state.auth_header_name = header_name.and_then(non_empty_string);
        state.auth_bearer_token = bearer_token.and_then(non_empty_string);
        state.updated_at_utc = unix_timestamp_secs();
    }

    async fn auth_state(&self) -> (Option<String>, bool) {
        let state = self.state.lock().await;
        (
            state.auth_header_name.clone(),
            state.auth_bearer_token.is_some(),
        )
    }
}

fn parse_inspect_transport(raw: &str) -> Result<InspectTransportKind, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "stdio" => Ok(InspectTransportKind::Stdio),
        "streamable-http" | "streamable_http" | "streamablehttp" | "http" => {
            Ok(InspectTransportKind::StreamableHttp)
        }
        "sse" => Ok(InspectTransportKind::Sse),
        "ws" | "wss" | "websocket" => Ok(InspectTransportKind::Ws),
        "grpc" => Ok(InspectTransportKind::Grpc),
        _ => Err(format!(
            "unsupported inspect transport '{raw}'. supported values: stdio, streamable-http, sse, ws, grpc"
        )),
    }
}

fn default_session_name(transport: InspectTransportKind, endpoint: &str) -> String {
    format!("{}:{}", transport.as_str(), endpoint)
}

fn empty_json_object() -> Value {
    serde_json::json!({})
}

fn map_tool_descriptor(metadata: ToolMetadata) -> InspectToolDescriptor {
    InspectToolDescriptor {
        name: metadata.name,
        description: metadata.description,
        input_schema: metadata.input_schema,
    }
}

fn status_for_tool_error(err: &ToolCallError) -> StatusCode {
    match err {
        ToolCallError::InvalidEndpoint(_)
        | ToolCallError::InvalidArguments(_)
        | ToolCallError::MissingRequired { .. } => StatusCode::BAD_REQUEST,
        ToolCallError::ToolNotFound { .. } => StatusCode::NOT_FOUND,
        ToolCallError::AuthorizationRequired { .. } => StatusCode::UNAUTHORIZED,
        ToolCallError::Protocol(_) | ToolCallError::Transport(_) => StatusCode::BAD_GATEWAY,
    }
}

fn session_not_found_response(session_id: &str) -> Response {
    json_error(
        StatusCode::NOT_FOUND,
        &format!("inspect session '{session_id}' was not found"),
    )
}

fn parse_call_arguments(arguments: Value) -> Result<Value, String> {
    if arguments.is_object() {
        Ok(arguments)
    } else {
        Err("tool call arguments must be a JSON object".to_string())
    }
}

fn non_empty_string(value: String) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn to_json_value<T: Serialize>(value: &T) -> Option<Value> {
    serde_json::to_value(value).ok()
}

pub async fn run(config: WebConfig) -> Result<(), String> {
    let _telemetry = init_telemetry(config.log_level, OutputTransport::Stdio, "web", "web");

    if config.hot_reload && config.hot_reload_port == config.port {
        return Err(format!(
            "--hot-reload-port ({}) must differ from --port ({})",
            config.hot_reload_port, config.port
        ));
    }

    let host: IpAddr = config
        .host
        .parse()
        .map_err(|err| format!("Invalid --host '{}': {err}", config.host))?;
    let addr = SocketAddr::new(host, config.port);

    let log_path = config.log_file.clone().unwrap_or_else(default_log_path);
    ensure_log_file(&log_path)?;

    let cache_file = config
        .theme_cache_file
        .clone()
        .unwrap_or_else(default_theme_cache_path);

    let themes = ThemeService {
        catalog_url: config.theme_catalog_url.clone(),
        cache_file,
        cache_ttl_seconds: config.theme_cache_ttl_seconds,
        client: reqwest::Client::new(),
        lock: Arc::new(Mutex::new(())),
    };

    let admin_proxy = config
        .admin_base_url
        .clone()
        .map(|url| AdminProxy::new(url, config.admin_token.clone()));

    let (log_sender, _) = broadcast::channel(LOG_STREAM_BUFFER);

    let state = AppState {
        log_path: log_path.clone(),
        log_sender: log_sender.clone(),
        auth_token: config.auth_token.clone(),
        admin_proxy,
        themes,
        inspect: InspectManager::new(INSPECT_MAX_SESSIONS, INSPECT_MAX_HISTORY_PER_SESSION),
        hot_reload_url: config
            .hot_reload
            .then(|| format!("http://127.0.0.1:{}", config.hot_reload_port)),
    };

    tokio::spawn(spawn_log_tailer(log_path.clone(), log_sender));

    let api_router = Router::new()
        .route("/health", get(api_health))
        .route("/logs/recent", get(api_logs_recent))
        .route("/logs/ws", get(api_logs_ws))
        .route("/inspect/connect", post(api_inspect_connect))
        .route("/inspect/disconnect", post(api_inspect_disconnect))
        .route("/inspect/session", get(api_inspect_session))
        .route("/inspect/sessions", get(api_inspect_sessions))
        .route("/inspect/tools/list", post(api_inspect_tools_list))
        .route("/inspect/tools/call", post(api_inspect_tools_call))
        .route("/inspect/prompts/list", post(api_inspect_prompts_list))
        .route("/inspect/prompts/get", post(api_inspect_prompts_get))
        .route("/inspect/resources/list", post(api_inspect_resources_list))
        .route(
            "/inspect/resources/templates/list",
            post(api_inspect_resource_templates_list),
        )
        .route("/inspect/resources/read", post(api_inspect_resources_read))
        .route(
            "/inspect/resources/subscribe",
            post(api_inspect_resources_subscribe),
        )
        .route(
            "/inspect/resources/unsubscribe",
            post(api_inspect_resources_unsubscribe),
        )
        .route("/inspect/ping", post(api_inspect_ping))
        .route("/inspect/roots/list", post(api_inspect_roots_list))
        .route("/inspect/roots/set", post(api_inspect_roots_set))
        .route(
            "/inspect/roots/list-changed",
            post(api_inspect_roots_list_changed),
        )
        .route("/inspect/tasks/list", post(api_inspect_tasks_list))
        .route("/inspect/tasks/get", post(api_inspect_tasks_get))
        .route("/inspect/tasks/result", post(api_inspect_tasks_result))
        .route("/inspect/tasks/cancel", post(api_inspect_tasks_cancel))
        .route("/inspect/history", get(api_inspect_history))
        .route("/inspect/history/clear", post(api_inspect_history_clear))
        .route("/inspect/notifications", get(api_inspect_notifications))
        .route(
            "/inspect/notifications/clear",
            post(api_inspect_notifications_clear),
        )
        .route("/inspect/metadata", get(api_inspect_metadata))
        .route("/inspect/metadata/set", post(api_inspect_metadata_set))
        .route("/inspect/auth/state", get(api_inspect_auth_state))
        .route("/inspect/auth/set", post(api_inspect_auth_set))
        .route("/runtime/health", get(api_runtime_health))
        .route("/runtime/metrics", get(api_runtime_metrics))
        .route("/runtime/sessions", get(api_runtime_sessions))
        .route("/discovery/search", post(api_discovery_search))
        .route("/themes/catalog", get(api_theme_catalog))
        .route("/themes/refresh", post(api_theme_refresh))
        .with_state(state.clone())
        .layer(middleware::from_fn_with_state(state.clone(), authorize_api));

    let app = if config.hot_reload {
        Router::new()
            .nest("/api", api_router)
            .route("/", get(hot_reload_index))
            .route("/assets/{*path}", get(hot_reload_passthrough))
            .fallback(get(hot_reload_fallback))
            .with_state(state)
    } else {
        Router::new()
            .nest("/api", api_router)
            .route("/", get(static_index))
            .route("/assets/{*path}", get(static_asset))
            .fallback(get(static_fallback))
            .with_state(state)
    };

    let _hot_reload_process = if config.hot_reload {
        Some(spawn_hot_reload_dev_server(&config)?)
    } else {
        None
    };

    let listen_url = format!("http://{}:{}", config.host, config.port);
    let inspector_url = if config.hot_reload {
        format!("http://127.0.0.1:{}", config.hot_reload_port)
    } else {
        listen_url.clone()
    };
    tracing::info!("Starting mcpway web inspector at {inspector_url}");
    tracing::info!("Using log file: {}", log_path.display());
    if config.hot_reload {
        tracing::info!("Hot reload enabled. API server listening at {listen_url}");
    }

    if !config.no_open_browser {
        if should_attempt_browser_launch() {
            let _ = try_open_browser(&inspector_url);
        } else {
            tracing::info!("Skipping automatic browser launch: no graphical session detected");
        }
    }

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|err| format!("Failed to bind {listen_url}: {err}"))?;
    axum::serve(listener, app.into_make_service())
        .await
        .map_err(|err| format!("Web server error: {err}"))
}

fn default_theme_cache_path() -> PathBuf {
    if let Some(home) = user_home_dir() {
        return home.join(".mcpway").join("themes").join("catalog.json");
    }
    PathBuf::from(".mcpway/themes/catalog.json")
}

async fn authorize_api(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let Some(expected) = state.auth_token.as_deref() else {
        return next.run(req).await;
    };

    if token_matches(req.headers(), req.uri(), expected) {
        return next.run(req).await;
    }

    json_error(StatusCode::UNAUTHORIZED, "missing or invalid bearer token")
}

fn token_matches(headers: &HeaderMap, uri: &Uri, expected: &str) -> bool {
    if let Some(value) = headers.get(axum::http::header::AUTHORIZATION) {
        if let Ok(raw) = value.to_str() {
            if let Some(token) = raw.strip_prefix("Bearer ") {
                if token.trim() == expected {
                    return true;
                }
            }
        }
    }

    if let Some(query) = uri.query() {
        for (key, value) in url::form_urlencoded::parse(query.as_bytes()) {
            if key == "token" && value == expected {
                return true;
            }
        }
    }

    false
}

async fn api_health(State(state): State<AppState>) -> impl IntoResponse {
    let inspect_sessions = state.inspect.session_count().await;
    Json(serde_json::json!({
        "status": "ok",
        "auth_enabled": state.auth_token.is_some(),
        "runtime_admin_enabled": state.admin_proxy.is_some(),
        "inspect_sessions": inspect_sessions,
        "log_path": state.log_path,
    }))
}

async fn api_logs_recent(
    State(state): State<AppState>,
    Query(query): Query<LogsRecentQuery>,
) -> impl IntoResponse {
    match read_recent_logs(&state.log_path, &query) {
        Ok(records) => Json(serde_json::json!({ "records": records })).into_response(),
        Err(err) => json_error(StatusCode::INTERNAL_SERVER_ERROR, &err),
    }
}

async fn api_logs_ws(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_logs_socket(socket, state.log_sender.subscribe()))
}

async fn api_inspect_connect(
    State(state): State<AppState>,
    Json(payload): Json<InspectConnectRequest>,
) -> impl IntoResponse {
    match state.inspect.connect(payload).await {
        Ok(response) => Json(response).into_response(),
        Err((status, message)) => json_error(status, &message),
    }
}

async fn api_inspect_disconnect(
    State(state): State<AppState>,
    Json(payload): Json<InspectDisconnectRequest>,
) -> impl IntoResponse {
    let Some(session) = state.inspect.remove_session(&payload.session_id).await else {
        return session_not_found_response(&payload.session_id);
    };

    session.mark_disconnected().await;

    Json(InspectDisconnectResponse {
        session_id: payload.session_id,
        disconnected: true,
    })
    .into_response()
}

async fn api_inspect_session(
    State(state): State<AppState>,
    Query(query): Query<InspectSessionQuery>,
) -> impl IntoResponse {
    let Some(session) = state.inspect.get_session(&query.session_id).await else {
        return session_not_found_response(&query.session_id);
    };
    Json(session.summary().await).into_response()
}

async fn api_inspect_sessions(State(state): State<AppState>) -> impl IntoResponse {
    Json(serde_json::json!({
        "sessions": state.inspect.list_sessions().await,
    }))
    .into_response()
}

async fn api_inspect_tools_list(
    State(state): State<AppState>,
    Json(payload): Json<InspectToolsListRequest>,
) -> impl IntoResponse {
    let Some(session) = state.inspect.get_session(&payload.session_id).await else {
        return session_not_found_response(&payload.session_id);
    };

    let request_payload = serde_json::json!({
        "session_id": payload.session_id,
    });
    match session.list_tools().await {
        Ok(response) => {
            session.mark_connected().await;
            session
                .push_history(
                    "tools.list",
                    format!("Listed {} tools", response.tool_count),
                    Some(request_payload),
                    to_json_value(&response),
                    None,
                )
                .await;
            Json(response).into_response()
        }
        Err(err) => {
            let message = err.to_string();
            session.mark_error(message.clone()).await;
            session
                .push_history(
                    "tools.list.error",
                    "Failed to list tools",
                    Some(request_payload),
                    None,
                    Some(message.clone()),
                )
                .await;
            json_error(status_for_tool_error(&err), &message)
        }
    }
}

async fn api_inspect_tools_call(
    State(state): State<AppState>,
    Json(payload): Json<InspectToolCallRequest>,
) -> impl IntoResponse {
    let Some(session) = state.inspect.get_session(&payload.session_id).await else {
        return session_not_found_response(&payload.session_id);
    };

    let arguments = match parse_call_arguments(payload.arguments) {
        Ok(arguments) => arguments,
        Err(message) => return json_error(StatusCode::BAD_REQUEST, &message),
    };

    let request_payload = serde_json::json!({
        "session_id": payload.session_id,
        "tool_name": payload.tool_name,
        "arguments": arguments,
    });
    let tool_name = request_payload
        .get("tool_name")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();
    let call_args = request_payload
        .get("arguments")
        .cloned()
        .unwrap_or_else(empty_json_object);

    match session.call_tool(&tool_name, call_args).await {
        Ok(response) => {
            session.mark_connected().await;
            session
                .push_history(
                    "tools.call",
                    format!("Called tool '{tool_name}'"),
                    Some(request_payload),
                    to_json_value(&response),
                    None,
                )
                .await;
            Json(response).into_response()
        }
        Err(err) => {
            let message = err.to_string();
            session.mark_error(message.clone()).await;
            session
                .push_history(
                    "tools.call.error",
                    format!("Failed to call tool '{tool_name}'"),
                    Some(request_payload),
                    None,
                    Some(message.clone()),
                )
                .await;
            json_error(status_for_tool_error(&err), &message)
        }
    }
}

async fn api_inspect_prompts_list(
    State(state): State<AppState>,
    Json(payload): Json<InspectPromptsListRequest>,
) -> impl IntoResponse {
    let params = match payload.cursor {
        Some(cursor) => serde_json::json!({ "cursor": cursor }),
        None => empty_json_object(),
    };
    inspect_rpc_call(state, payload.session_id, "prompts/list", params).await
}

async fn api_inspect_prompts_get(
    State(state): State<AppState>,
    Json(payload): Json<InspectPromptGetRequest>,
) -> impl IntoResponse {
    if payload.name.trim().is_empty() {
        return json_error(StatusCode::BAD_REQUEST, "prompt name cannot be empty");
    }

    if !payload.arguments.is_object() {
        return json_error(
            StatusCode::BAD_REQUEST,
            "prompt arguments must be a JSON object",
        );
    }

    let params = serde_json::json!({
        "name": payload.name,
        "arguments": payload.arguments,
    });
    inspect_rpc_call(state, payload.session_id, "prompts/get", params).await
}

async fn api_inspect_resources_list(
    State(state): State<AppState>,
    Json(payload): Json<InspectResourcesListRequest>,
) -> impl IntoResponse {
    let params = match payload.cursor {
        Some(cursor) => serde_json::json!({ "cursor": cursor }),
        None => empty_json_object(),
    };
    inspect_rpc_call(state, payload.session_id, "resources/list", params).await
}

async fn api_inspect_resource_templates_list(
    State(state): State<AppState>,
    Json(payload): Json<InspectResourceTemplatesListRequest>,
) -> impl IntoResponse {
    let params = match payload.cursor {
        Some(cursor) => serde_json::json!({ "cursor": cursor }),
        None => empty_json_object(),
    };
    inspect_rpc_call(
        state,
        payload.session_id,
        "resources/templates/list",
        params,
    )
    .await
}

async fn api_inspect_resources_read(
    State(state): State<AppState>,
    Json(payload): Json<InspectResourceReadRequest>,
) -> impl IntoResponse {
    if payload.uri.trim().is_empty() {
        return json_error(StatusCode::BAD_REQUEST, "resource uri cannot be empty");
    }
    let params = serde_json::json!({ "uri": payload.uri });
    inspect_rpc_call(state, payload.session_id, "resources/read", params).await
}

async fn api_inspect_resources_subscribe(
    State(state): State<AppState>,
    Json(payload): Json<InspectResourceSubscriptionRequest>,
) -> impl IntoResponse {
    if payload.uri.trim().is_empty() {
        return json_error(StatusCode::BAD_REQUEST, "resource uri cannot be empty");
    }
    let params = serde_json::json!({ "uri": payload.uri });
    inspect_rpc_call(state, payload.session_id, "resources/subscribe", params).await
}

async fn api_inspect_resources_unsubscribe(
    State(state): State<AppState>,
    Json(payload): Json<InspectResourceSubscriptionRequest>,
) -> impl IntoResponse {
    if payload.uri.trim().is_empty() {
        return json_error(StatusCode::BAD_REQUEST, "resource uri cannot be empty");
    }
    let params = serde_json::json!({ "uri": payload.uri });
    inspect_rpc_call(state, payload.session_id, "resources/unsubscribe", params).await
}

async fn api_inspect_ping(
    State(state): State<AppState>,
    Json(payload): Json<InspectPingRequest>,
) -> impl IntoResponse {
    inspect_rpc_call(state, payload.session_id, "ping", empty_json_object()).await
}

async fn api_inspect_roots_list(
    State(state): State<AppState>,
    Json(payload): Json<InspectSessionQuery>,
) -> impl IntoResponse {
    inspect_rpc_call(state, payload.session_id, "roots/list", empty_json_object()).await
}

async fn api_inspect_roots_set(
    State(state): State<AppState>,
    Json(payload): Json<InspectRootsSetRequest>,
) -> impl IntoResponse {
    inspect_rpc_notify(
        state,
        payload.session_id,
        "roots/list",
        serde_json::json!({ "roots": payload.roots }),
        "roots.set",
        "Sent roots/list payload",
    )
    .await
}

async fn api_inspect_roots_list_changed(
    State(state): State<AppState>,
    Json(payload): Json<InspectSessionQuery>,
) -> impl IntoResponse {
    inspect_rpc_notify(
        state,
        payload.session_id,
        "notifications/roots/list_changed",
        empty_json_object(),
        "roots.list_changed",
        "Sent roots/list_changed notification",
    )
    .await
}

async fn api_inspect_tasks_list(
    State(state): State<AppState>,
    Json(payload): Json<InspectSessionQuery>,
) -> impl IntoResponse {
    inspect_rpc_call(state, payload.session_id, "tasks/list", empty_json_object()).await
}

async fn api_inspect_tasks_get(
    State(state): State<AppState>,
    Json(payload): Json<InspectTaskByIdRequest>,
) -> impl IntoResponse {
    if payload.task_id.trim().is_empty() {
        return json_error(StatusCode::BAD_REQUEST, "task_id cannot be empty");
    }
    inspect_rpc_call(
        state,
        payload.session_id,
        "tasks/get",
        serde_json::json!({ "taskId": payload.task_id }),
    )
    .await
}

async fn api_inspect_tasks_result(
    State(state): State<AppState>,
    Json(payload): Json<InspectTaskByIdRequest>,
) -> impl IntoResponse {
    if payload.task_id.trim().is_empty() {
        return json_error(StatusCode::BAD_REQUEST, "task_id cannot be empty");
    }
    inspect_rpc_call(
        state,
        payload.session_id,
        "tasks/result",
        serde_json::json!({ "taskId": payload.task_id }),
    )
    .await
}

async fn api_inspect_tasks_cancel(
    State(state): State<AppState>,
    Json(payload): Json<InspectTaskByIdRequest>,
) -> impl IntoResponse {
    if payload.task_id.trim().is_empty() {
        return json_error(StatusCode::BAD_REQUEST, "task_id cannot be empty");
    }
    inspect_rpc_call(
        state,
        payload.session_id,
        "tasks/cancel",
        serde_json::json!({ "taskId": payload.task_id }),
    )
    .await
}

async fn api_inspect_history(
    State(state): State<AppState>,
    Query(query): Query<InspectHistoryQuery>,
) -> impl IntoResponse {
    let Some(session) = state.inspect.get_session(&query.session_id).await else {
        return session_not_found_response(&query.session_id);
    };

    let entries = session.history(query.limit).await;
    Json(InspectHistoryResponse {
        session_id: query.session_id,
        count: entries.len(),
        entries,
    })
    .into_response()
}

async fn api_inspect_history_clear(
    State(state): State<AppState>,
    Json(payload): Json<InspectSessionQuery>,
) -> impl IntoResponse {
    let Some(session) = state.inspect.get_session(&payload.session_id).await else {
        return session_not_found_response(&payload.session_id);
    };

    let cleared = session.clear_history().await;
    Json(InspectHistoryClearResponse {
        session_id: payload.session_id,
        cleared,
    })
    .into_response()
}

async fn api_inspect_notifications(
    State(state): State<AppState>,
    Query(query): Query<InspectNotificationsQuery>,
) -> impl IntoResponse {
    let Some(session) = state.inspect.get_session(&query.session_id).await else {
        return session_not_found_response(&query.session_id);
    };

    let entries = session.notifications(query.limit).await;
    Json(InspectNotificationsResponse {
        session_id: query.session_id,
        count: entries.len(),
        entries,
    })
    .into_response()
}

async fn api_inspect_notifications_clear(
    State(state): State<AppState>,
    Json(payload): Json<InspectSessionQuery>,
) -> impl IntoResponse {
    let Some(session) = state.inspect.get_session(&payload.session_id).await else {
        return session_not_found_response(&payload.session_id);
    };

    let cleared = session.clear_notifications().await;
    Json(serde_json::json!({
        "session_id": payload.session_id,
        "cleared": cleared,
    }))
    .into_response()
}

async fn api_inspect_metadata(
    State(state): State<AppState>,
    Query(query): Query<InspectSessionQuery>,
) -> impl IntoResponse {
    let Some(session) = state.inspect.get_session(&query.session_id).await else {
        return session_not_found_response(&query.session_id);
    };

    Json(InspectMetadataResponse {
        session_id: query.session_id,
        metadata: session.metadata().await,
    })
    .into_response()
}

async fn api_inspect_metadata_set(
    State(state): State<AppState>,
    Json(payload): Json<InspectMetadataSetRequest>,
) -> impl IntoResponse {
    let Some(session) = state.inspect.get_session(&payload.session_id).await else {
        return session_not_found_response(&payload.session_id);
    };
    session.set_metadata(payload.metadata.clone()).await;
    session
        .push_history(
            "metadata.set",
            "Updated session metadata",
            Some(serde_json::json!({ "metadata": payload.metadata })),
            None,
            None,
        )
        .await;
    Json(InspectMetadataResponse {
        session_id: payload.session_id,
        metadata: session.metadata().await,
    })
    .into_response()
}

async fn api_inspect_auth_state(
    State(state): State<AppState>,
    Query(query): Query<InspectSessionQuery>,
) -> impl IntoResponse {
    let Some(session) = state.inspect.get_session(&query.session_id).await else {
        return session_not_found_response(&query.session_id);
    };

    let (header_name, has_bearer_token) = session.auth_state().await;
    Json(InspectAuthStateResponse {
        session_id: query.session_id,
        header_name,
        has_bearer_token,
    })
    .into_response()
}

async fn api_inspect_auth_set(
    State(state): State<AppState>,
    Json(payload): Json<InspectAuthSetRequest>,
) -> impl IntoResponse {
    let Some(session) = state.inspect.get_session(&payload.session_id).await else {
        return session_not_found_response(&payload.session_id);
    };
    session
        .set_auth_state(payload.header_name.clone(), payload.bearer_token.clone())
        .await;
    session
        .push_history(
            "auth.set",
            "Updated session auth settings",
            Some(serde_json::json!({
                "header_name": payload.header_name,
                "has_bearer_token": payload
                    .bearer_token
                    .as_deref()
                    .map(|value| !value.trim().is_empty())
                    .unwrap_or(false),
            })),
            None,
            None,
        )
        .await;
    let (header_name, has_bearer_token) = session.auth_state().await;
    Json(InspectAuthStateResponse {
        session_id: payload.session_id,
        header_name,
        has_bearer_token,
    })
    .into_response()
}

async fn inspect_rpc_call(
    state: AppState,
    session_id: String,
    method: &'static str,
    params: Value,
) -> Response {
    let Some(session) = state.inspect.get_session(&session_id).await else {
        return session_not_found_response(&session_id);
    };

    let request_payload = serde_json::json!({
        "session_id": session_id,
        "method": method,
        "params": params,
    });

    let call_params = request_payload
        .get("params")
        .cloned()
        .unwrap_or_else(empty_json_object);
    match session.request_method(method, call_params).await {
        Ok(response) => {
            session.mark_connected().await;
            session
                .push_history(
                    "rpc.call",
                    format!("Called method '{method}'"),
                    Some(request_payload),
                    to_json_value(&response),
                    None,
                )
                .await;
            Json(response).into_response()
        }
        Err(err) => {
            let message = err.to_string();
            session.mark_error(message.clone()).await;
            session
                .push_history(
                    "rpc.call.error",
                    format!("Failed method '{method}'"),
                    Some(request_payload),
                    None,
                    Some(message.clone()),
                )
                .await;
            json_error(status_for_tool_error(&err), &message)
        }
    }
}

async fn inspect_rpc_notify(
    state: AppState,
    session_id: String,
    method: &'static str,
    params: Value,
    history_kind: &'static str,
    history_summary: &'static str,
) -> Response {
    let Some(session) = state.inspect.get_session(&session_id).await else {
        return session_not_found_response(&session_id);
    };

    let request_payload = serde_json::json!({
        "session_id": session_id,
        "method": method,
        "params": params,
    });
    let notify_params = request_payload
        .get("params")
        .cloned()
        .unwrap_or_else(empty_json_object);
    match session.notify_method(method, notify_params.clone()).await {
        Ok(()) => {
            session.mark_connected().await;
            session
                .push_history(
                    history_kind,
                    history_summary,
                    Some(request_payload),
                    Some(serde_json::json!({"status": "sent"})),
                    None,
                )
                .await;
            session
                .push_notification(method, notify_params, history_summary)
                .await;
            Json(serde_json::json!({
                "session_id": session_id,
                "method": method,
                "status": "sent",
            }))
            .into_response()
        }
        Err(err) => {
            let message = err.to_string();
            let history_kind_error = format!("{history_kind}.error");
            session.mark_error(message.clone()).await;
            session
                .push_history(
                    history_kind_error.as_str(),
                    format!("Failed to send notification '{method}'"),
                    Some(request_payload),
                    None,
                    Some(message.clone()),
                )
                .await;
            json_error(status_for_tool_error(&err), &message)
        }
    }
}

async fn handle_logs_socket(socket: WebSocket, mut rx: broadcast::Receiver<StoredLogRecord>) {
    let (mut sender, mut receiver) = socket.split();

    let send_task = tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(record) => {
                    let Ok(line) = serde_json::to_string(&record) else {
                        continue;
                    };
                    if sender.send(Message::Text(line.into())).await.is_err() {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });

    while let Some(message) = receiver.next().await {
        match message {
            Ok(Message::Close(_)) | Err(_) => break,
            _ => {}
        }
    }

    send_task.abort();
}

async fn api_runtime_health(State(state): State<AppState>) -> impl IntoResponse {
    forward_admin_get(state.admin_proxy.clone(), "/v1/runtime/health").await
}

async fn api_runtime_metrics(State(state): State<AppState>) -> impl IntoResponse {
    forward_admin_get(state.admin_proxy.clone(), "/v1/runtime/metrics").await
}

async fn api_runtime_sessions(State(state): State<AppState>) -> impl IntoResponse {
    forward_admin_get(state.admin_proxy.clone(), "/v1/runtime/sessions").await
}

async fn api_discovery_search(
    State(state): State<AppState>,
    Json(payload): Json<Value>,
) -> impl IntoResponse {
    let Some(proxy) = state.admin_proxy.as_ref() else {
        return Json(serde_json::json!({
            "status": "disabled",
            "reason": "admin_not_configured",
        }))
        .into_response();
    };

    match proxy.post_json("/v1/discovery/search", &payload).await {
        Ok(value) => Json(value).into_response(),
        Err(err) => json_error(StatusCode::BAD_GATEWAY, &err),
    }
}

async fn forward_admin_get(proxy: Option<AdminProxy>, path: &str) -> Response {
    let Some(proxy) = proxy.as_ref() else {
        return Json(serde_json::json!({
            "status": "disabled",
            "reason": "admin_not_configured",
        }))
        .into_response();
    };

    match proxy.get_json(path).await {
        Ok(value) => Json(value).into_response(),
        Err(err) => json_error(StatusCode::BAD_GATEWAY, &err),
    }
}

async fn api_theme_catalog(State(state): State<AppState>) -> impl IntoResponse {
    match state.themes.load_catalog(false).await {
        Ok(catalog) => Json(catalog).into_response(),
        Err(err) => json_error(StatusCode::BAD_GATEWAY, &err),
    }
}

async fn api_theme_refresh(State(state): State<AppState>) -> impl IntoResponse {
    match state.themes.load_catalog(true).await {
        Ok(catalog) => Json(catalog).into_response(),
        Err(err) => json_error(StatusCode::BAD_GATEWAY, &err),
    }
}

impl ThemeService {
    async fn load_catalog(&self, force_refresh: bool) -> Result<ThemeCatalog, String> {
        let _guard = self.lock.lock().await;

        if !force_refresh {
            if let Some(cached) = self.read_cache_if_fresh()? {
                return Ok(cached);
            }
        }

        match self.fetch_catalog().await {
            Ok(themes) => {
                let catalog = ThemeCatalog {
                    fetched_at_utc: unix_timestamp_secs(),
                    themes,
                };
                self.write_cache(&catalog)?;
                Ok(catalog)
            }
            Err(fetch_err) => {
                if let Some(cached) = self.read_cache_any()? {
                    return Ok(cached);
                }

                let fallback = ThemeCatalog {
                    fetched_at_utc: unix_timestamp_secs(),
                    themes: vec![builtin_theme()],
                };
                tracing::warn!("Theme catalog fetch failed, using builtin fallback: {fetch_err}");
                Ok(fallback)
            }
        }
    }

    fn read_cache_if_fresh(&self) -> Result<Option<ThemeCatalog>, String> {
        let Some(cached) = self.read_cache_any()? else {
            return Ok(None);
        };
        let age = unix_timestamp_secs().saturating_sub(cached.fetched_at_utc);
        if age <= self.cache_ttl_seconds {
            Ok(Some(cached))
        } else {
            Ok(None)
        }
    }

    fn read_cache_any(&self) -> Result<Option<ThemeCatalog>, String> {
        if !self.cache_file.exists() {
            return Ok(None);
        }
        let raw = std::fs::read_to_string(&self.cache_file).map_err(|err| {
            format!(
                "Failed to read theme cache {}: {err}",
                self.cache_file.display()
            )
        })?;
        let parsed = serde_json::from_str::<ThemeCatalog>(&raw).map_err(|err| {
            format!(
                "Failed to parse theme cache {}: {err}",
                self.cache_file.display()
            )
        })?;
        Ok(Some(parsed))
    }

    fn write_cache(&self, catalog: &ThemeCatalog) -> Result<(), String> {
        if let Some(parent) = self.cache_file.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|err| format!("Failed to create cache dir {}: {err}", parent.display()))?;
        }
        let body = serde_json::to_string_pretty(catalog)
            .map_err(|err| format!("Failed to serialize theme cache: {err}"))?;
        std::fs::write(&self.cache_file, body)
            .map_err(|err| format!("Failed to write cache {}: {err}", self.cache_file.display()))
    }

    async fn fetch_catalog(&self) -> Result<Vec<ThemeDescriptor>, String> {
        let response = self
            .client
            .get(&self.catalog_url)
            .header("User-Agent", "mcpway-web")
            .send()
            .await
            .map_err(|err| format!("Failed to fetch theme catalog: {err}"))?;

        if !response.status().is_success() {
            return Err(format!(
                "Theme catalog returned HTTP {}",
                response.status().as_u16()
            ));
        }

        let body = response
            .text()
            .await
            .map_err(|err| format!("Failed to read theme catalog response: {err}"))?;
        let json = serde_json::from_str::<Value>(&body)
            .map_err(|err| format!("Theme catalog is not valid JSON: {err}"))?;

        if let Some(themes) = parse_prebuilt_theme_catalog(&json) {
            if !themes.is_empty() {
                return Ok(themes);
            }
        }

        let Some(entries) = parse_github_entries(&json) else {
            return Err("Theme catalog format is unsupported".to_string());
        };

        let mut themes = Vec::new();
        for entry in entries.into_iter().take(80) {
            let Some(download_url) = entry.download_url else {
                continue;
            };
            let response = self
                .client
                .get(&download_url)
                .header("User-Agent", "mcpway-web")
                .send()
                .await
                .map_err(|err| format!("Failed to fetch theme '{}': {err}", entry.name))?;
            if !response.status().is_success() {
                continue;
            }
            let xml = match response.text().await {
                Ok(value) => value,
                Err(_) => continue,
            };
            if let Some(theme) = parse_iterm_theme(&entry.name, &download_url, &xml) {
                themes.push(theme);
            }
        }

        if themes.is_empty() {
            return Err("No valid themes were parsed from the remote catalog".to_string());
        }

        themes.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(themes)
    }
}

#[derive(Debug, Deserialize)]
struct GitHubEntry {
    name: String,
    download_url: Option<String>,
    #[serde(rename = "type")]
    kind: String,
}

fn parse_github_entries(value: &Value) -> Option<Vec<GitHubEntry>> {
    let entries = serde_json::from_value::<Vec<GitHubEntry>>(value.clone()).ok()?;
    Some(
        entries
            .into_iter()
            .filter(|entry| entry.kind == "file" && entry.name.ends_with(".itermcolors"))
            .collect(),
    )
}

fn parse_prebuilt_theme_catalog(value: &Value) -> Option<Vec<ThemeDescriptor>> {
    let themes_value = value.get("themes")?;
    serde_json::from_value::<Vec<ThemeDescriptor>>(themes_value.clone()).ok()
}

fn parse_iterm_theme(name: &str, source_url: &str, xml: &str) -> Option<ThemeDescriptor> {
    let root = plist::Value::from_reader_xml(xml.as_bytes()).ok()?;
    let dict = root.as_dictionary()?;

    let background = parse_color(dict, "Background Color").unwrap_or_else(|| "#0f111a".to_string());
    let foreground = parse_color(dict, "Foreground Color").unwrap_or_else(|| "#d0d4de".to_string());
    let cursor = parse_color(dict, "Cursor Color").unwrap_or_else(|| foreground.clone());

    let mut ansi = Vec::with_capacity(16);
    for idx in 0..16 {
        let key = format!("Ansi {idx} Color");
        let color = parse_color(dict, &key).unwrap_or_else(|| {
            if idx < 8 {
                background.clone()
            } else {
                foreground.clone()
            }
        });
        ansi.push(color);
    }

    Some(ThemeDescriptor {
        id: slugify_theme_name(name),
        name: name.trim_end_matches(".itermcolors").to_string(),
        source_url: source_url.to_string(),
        palette: ThemePalette {
            background,
            foreground,
            cursor,
            ansi,
        },
    })
}

fn parse_color(dict: &plist::Dictionary, key: &str) -> Option<String> {
    let color = dict.get(key)?.as_dictionary()?;
    let r = color.get("Red Component")?.as_real()?;
    let g = color.get("Green Component")?.as_real()?;
    let b = color.get("Blue Component")?.as_real()?;
    Some(format!(
        "#{:02x}{:02x}{:02x}",
        float_channel_to_byte(r),
        float_channel_to_byte(g),
        float_channel_to_byte(b)
    ))
}

fn float_channel_to_byte(channel: f64) -> u8 {
    let scaled = (channel * 255.0).round();
    scaled.clamp(0.0, 255.0) as u8
}

fn slugify_theme_name(name: &str) -> String {
    let mut slug = String::with_capacity(name.len());
    for ch in name.trim_end_matches(".itermcolors").chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    slug.trim_matches('-').to_string()
}

fn builtin_theme() -> ThemeDescriptor {
    ThemeDescriptor {
        id: "mcpway-default".to_string(),
        name: "MCPway Default".to_string(),
        source_url: "builtin".to_string(),
        palette: ThemePalette {
            background: "#101114".to_string(),
            foreground: "#dde1ea".to_string(),
            cursor: "#f5f7fa".to_string(),
            ansi: vec![
                "#101114".to_string(),
                "#ff5f56".to_string(),
                "#27c93f".to_string(),
                "#ffbd2e".to_string(),
                "#4a90e2".to_string(),
                "#bd93f9".to_string(),
                "#00d0d0".to_string(),
                "#f5f7fa".to_string(),
                "#3a3f4b".to_string(),
                "#ff7b72".to_string(),
                "#3fb950".to_string(),
                "#d29922".to_string(),
                "#58a6ff".to_string(),
                "#bc8cff".to_string(),
                "#39c5cf".to_string(),
                "#ffffff".to_string(),
            ],
        },
    }
}

fn read_recent_logs(path: &Path, query: &LogsRecentQuery) -> Result<Vec<StoredLogRecord>, String> {
    let lines = query.lines.unwrap_or(300).clamp(1, 5000);
    let content = std::fs::read_to_string(path)
        .map_err(|err| format!("Failed to read {}: {err}", path.display()))?;

    let mut buffer = VecDeque::with_capacity(lines);
    for line in content.lines() {
        let Some(record) = parse_record(line) else {
            continue;
        };
        if !matches_log_filter(&record, query) {
            continue;
        }
        if buffer.len() >= lines {
            buffer.pop_front();
        }
        buffer.push_back(record);
    }

    Ok(buffer.into_iter().collect())
}

fn matches_log_filter(record: &StoredLogRecord, query: &LogsRecentQuery) -> bool {
    if let Some(level) = query.level.as_deref() {
        if record.level != level.to_ascii_lowercase() {
            return false;
        }
    }
    if let Some(transport) = query.transport.as_deref() {
        if record.transport != transport {
            return false;
        }
    }
    if let Some(search) = query.search.as_deref() {
        let search = search.to_ascii_lowercase();
        let message = record.message.to_ascii_lowercase();
        if !message.contains(&search) {
            return false;
        }
    }
    true
}

async fn spawn_log_tailer(path: PathBuf, sender: broadcast::Sender<StoredLogRecord>) {
    let mut offset = match std::fs::metadata(&path) {
        Ok(meta) => meta.len(),
        Err(_) => 0,
    };

    loop {
        tokio::time::sleep(Duration::from_millis(400)).await;

        let current_len = match std::fs::metadata(&path) {
            Ok(meta) => meta.len(),
            Err(_) => continue,
        };
        if current_len < offset {
            offset = 0;
        }

        let mut file = match File::open(&path) {
            Ok(file) => file,
            Err(_) => continue,
        };
        if file.seek(SeekFrom::Start(offset)).is_err() {
            continue;
        }

        let mut text = String::new();
        if file.read_to_string(&mut text).is_err() {
            continue;
        }

        match file.seek(SeekFrom::End(0)) {
            Ok(next_offset) => offset = next_offset,
            Err(_) => continue,
        }

        for line in text.lines() {
            if let Some(record) = parse_record(line) {
                let _ = sender.send(record);
            }
        }
    }
}

async fn static_index() -> Response {
    serve_embedded_file("index.html")
}

async fn static_asset(axum::extract::Path(path): axum::extract::Path<String>) -> Response {
    serve_embedded_file(&format!("assets/{path}"))
}

async fn static_fallback(uri: Uri) -> Response {
    if uri.path().starts_with("/api/") {
        return json_error(StatusCode::NOT_FOUND, "API route not found");
    }
    serve_embedded_file("index.html")
}

async fn hot_reload_index(State(state): State<AppState>) -> Response {
    redirect_to_hot_reload(&state, "/")
}

async fn hot_reload_passthrough(State(state): State<AppState>, uri: Uri) -> Response {
    let path_and_query = uri
        .path_and_query()
        .map(|value| value.as_str())
        .unwrap_or(uri.path());
    redirect_to_hot_reload(&state, path_and_query)
}

async fn hot_reload_fallback(State(state): State<AppState>, uri: Uri) -> Response {
    if uri.path().starts_with("/api/") {
        return json_error(StatusCode::NOT_FOUND, "API route not found");
    }

    let path_and_query = uri
        .path_and_query()
        .map(|value| value.as_str())
        .unwrap_or(uri.path());
    redirect_to_hot_reload(&state, path_and_query)
}

fn redirect_to_hot_reload(state: &AppState, path_and_query: &str) -> Response {
    let Some(base) = state.hot_reload_url.as_deref() else {
        return json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "hot reload is not configured for this process",
        );
    };

    let mut base_url = base.trim_end_matches('/').to_string();
    let suffix = if path_and_query.starts_with('/') {
        path_and_query
    } else {
        "/"
    };
    base_url.push_str(suffix);
    if !path_and_query.starts_with('/') {
        base_url.push_str(path_and_query);
    }
    Redirect::temporary(&base_url).into_response()
}

fn serve_embedded_file(path: &str) -> Response {
    let normalized = path.trim_start_matches('/');
    let Some(file) = WEB_DIST.get_file(normalized) else {
        return json_error(StatusCode::NOT_FOUND, "asset not found");
    };

    let mime = mime_guess::from_path(normalized).first_or_octet_stream();
    (
        StatusCode::OK,
        [(axum::http::header::CONTENT_TYPE, mime.as_ref())],
        file.contents().to_vec(),
    )
        .into_response()
}

fn json_error(status: StatusCode, message: &str) -> Response {
    (
        status,
        Json(serde_json::json!({
            "status": "error",
            "message": message,
        })),
    )
        .into_response()
}

struct ChildProcessGuard {
    child: Option<Child>,
}

impl Drop for ChildProcessGuard {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn spawn_hot_reload_dev_server(config: &WebConfig) -> Result<ChildProcessGuard, String> {
    let web_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../web");
    let port = config.hot_reload_port.to_string();
    let api_url = if config.host == "0.0.0.0" {
        format!("http://127.0.0.1:{}", config.port)
    } else {
        format!("http://{}:{}", config.host, config.port)
    };
    let child = Command::new("npm")
        .args([
            "run",
            "dev",
            "--",
            "--host",
            "127.0.0.1",
            "--port",
            port.as_str(),
            "--strictPort",
        ])
        .current_dir(&web_dir)
        .env("MCPWAY_API_BASE", api_url)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|err| {
            format!(
                "Failed to launch Vite dev server from {}: {err}",
                web_dir.display()
            )
        })?;
    Ok(ChildProcessGuard { child: Some(child) })
}

fn unix_timestamp_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_inspect_transport_accepts_aliases() {
        assert!(matches!(
            parse_inspect_transport("stdio"),
            Ok(InspectTransportKind::Stdio)
        ));
        assert!(matches!(
            parse_inspect_transport("streamable-http"),
            Ok(InspectTransportKind::StreamableHttp)
        ));
        assert!(matches!(
            parse_inspect_transport("HTTP"),
            Ok(InspectTransportKind::StreamableHttp)
        ));
        assert!(matches!(
            parse_inspect_transport("wss"),
            Ok(InspectTransportKind::Ws)
        ));
        assert!(matches!(
            parse_inspect_transport("grpc"),
            Ok(InspectTransportKind::Grpc)
        ));
        assert!(parse_inspect_transport("invalid").is_err());
    }

    #[test]
    fn slugify_theme_name_normalizes_characters() {
        assert_eq!(slugify_theme_name("Tokyo Night.itermcolors"), "tokyo-night");
        assert_eq!(slugify_theme_name("One  Dark++"), "one-dark");
    }

    #[test]
    fn token_matching_accepts_query_param() {
        let headers = HeaderMap::new();
        let uri = Uri::from_static("/api/logs/ws?token=secret");
        assert!(token_matches(&headers, &uri, "secret"));
    }

    #[test]
    fn builtin_theme_has_full_ansi_palette() {
        let theme = builtin_theme();
        assert_eq!(theme.palette.ansi.len(), 16);
    }

    #[tokio::test]
    async fn inspect_session_history_respects_capacity() {
        let client = ToolClientBuilder::new("http://127.0.0.1:1", Transport::StreamableHttp)
            .build()
            .expect("build tool client");
        let session = InspectSession::new(
            "test-session".to_string(),
            "test".to_string(),
            InspectTransportKind::StreamableHttp,
            "http://127.0.0.1:1".to_string(),
            2,
            client,
        );

        session
            .push_history("first", "first entry", None, None, None)
            .await;
        session
            .push_history("second", "second entry", None, None, None)
            .await;
        session
            .push_history("third", "third entry", None, None, None)
            .await;

        let entries = session.history(None).await;
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].summary, "second entry");
        assert_eq!(entries[1].summary, "third entry");
    }
}
