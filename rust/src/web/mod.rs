use std::collections::VecDeque;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, Request, State};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures::{SinkExt, StreamExt};
use include_dir::{include_dir, Dir};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{broadcast, Mutex};

use crate::config::{OutputTransport, WebConfig};
use crate::discovery::user_home_dir;
use crate::support::log_store::{default_log_path, ensure_log_file, parse_record, StoredLogRecord};
use crate::support::telemetry::init_telemetry;

static WEB_DIST: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/../web/dist");
const LOG_STREAM_BUFFER: usize = 2048;

#[derive(Clone)]
struct AppState {
    log_path: PathBuf,
    log_sender: broadcast::Sender<StoredLogRecord>,
    auth_token: Option<String>,
    admin_proxy: Option<AdminProxy>,
    themes: ThemeService,
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

pub async fn run(config: WebConfig) -> Result<(), String> {
    let _telemetry = init_telemetry(config.log_level, OutputTransport::Stdio, "web", "web");

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
    };

    tokio::spawn(spawn_log_tailer(log_path.clone(), log_sender));

    let api_router = Router::new()
        .route("/health", get(api_health))
        .route("/logs/recent", get(api_logs_recent))
        .route("/logs/ws", get(api_logs_ws))
        .route("/runtime/health", get(api_runtime_health))
        .route("/runtime/metrics", get(api_runtime_metrics))
        .route("/runtime/sessions", get(api_runtime_sessions))
        .route("/discovery/search", post(api_discovery_search))
        .route("/themes/catalog", get(api_theme_catalog))
        .route("/themes/refresh", post(api_theme_refresh))
        .with_state(state.clone())
        .layer(middleware::from_fn_with_state(state.clone(), authorize_api));

    let app = Router::new()
        .nest("/api", api_router)
        .route("/", get(static_index))
        .route("/assets/*path", get(static_asset))
        .fallback(get(static_fallback))
        .with_state(state);

    let listen_url = format!("http://{}:{}", config.host, config.port);
    tracing::info!("Starting mcpway web inspector at {listen_url}");
    tracing::info!("Using log file: {}", log_path.display());

    if !config.no_open_browser {
        let _ = try_open_browser(&listen_url);
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
        return home
            .join(".mcpway")
            .join("themes")
            .join("catalog.json");
    }
    PathBuf::from(".mcpway/themes/catalog.json")
}

async fn authorize_api(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Response {
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
    Json(serde_json::json!({
        "status": "ok",
        "auth_enabled": state.auth_token.is_some(),
        "runtime_admin_enabled": state.admin_proxy.is_some(),
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
        let raw = std::fs::read_to_string(&self.cache_file)
            .map_err(|err| format!("Failed to read theme cache {}: {err}", self.cache_file.display()))?;
        let parsed = serde_json::from_str::<ThemeCatalog>(&raw)
            .map_err(|err| format!("Failed to parse theme cache {}: {err}", self.cache_file.display()))?;
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

fn unix_timestamp_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn try_open_browser(url: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(url)
            .status()
            .map_err(|err| format!("Failed to launch browser via open: {err}"))?;
        return Ok(());
    }

    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(["/C", "start", "", url])
            .status()
            .map_err(|err| format!("Failed to launch browser via cmd /C start: {err}"))?;
        return Ok(());
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::process::Command::new("xdg-open")
            .arg(url)
            .status()
            .map_err(|err| format!("Failed to launch browser via xdg-open: {err}"))?;
        return Ok(());
    }

    #[allow(unreachable_code)]
    Err("Automatic browser open is unsupported on this platform".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
