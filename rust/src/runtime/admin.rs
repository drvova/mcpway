use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::discovery::{self, DiscoverOptions, DiscoverySearchOptions, SourceKind};
use crate::runtime::store::{RuntimeArgsStore, RuntimeArgsUpdate};
use crate::runtime::{RuntimeApplyResult, RuntimeScope, RuntimeUpdate};
use axum::body::Body;
use axum::extract::{ConnectInfo, MatchedPath, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

#[derive(Clone, Debug, Default)]
pub struct AdminServerOptions {
    pub bearer_token: Option<String>,
    pub loopback_only: bool,
    pub discovery_project_root: Option<PathBuf>,
    pub discovery_source: Option<SourceKind>,
}

#[derive(Clone)]
pub struct AdminState {
    runtime: RuntimeArgsStore,
    handler: Arc<dyn Fn(RuntimeUpdate) -> BoxFuture<'static, RuntimeApplyResult> + Send + Sync>,
    options: AdminServerOptions,
    metrics: Arc<AdminMetrics>,
}

#[derive(Debug, Default)]
struct AdminMetrics {
    requests_total: AtomicU64,
    unauthorized_total: AtomicU64,
    forbidden_total: AtomicU64,
    runtime_updates_total: AtomicU64,
    discovery_search_total: AtomicU64,
    route_requests: Mutex<BTreeMap<String, u64>>,
    status_counts: Mutex<BTreeMap<u16, u64>>,
}

#[derive(Debug, Clone, Serialize)]
struct AdminMetricsSnapshot {
    requests_total: u64,
    unauthorized_total: u64,
    forbidden_total: u64,
    runtime_updates_total: u64,
    discovery_search_total: u64,
    route_requests: BTreeMap<String, u64>,
    status_counts: BTreeMap<String, u64>,
}

impl AdminMetrics {
    async fn record_request(&self, route: &str, status: StatusCode) {
        self.requests_total.fetch_add(1, Ordering::Relaxed);
        if status == StatusCode::UNAUTHORIZED {
            self.unauthorized_total.fetch_add(1, Ordering::Relaxed);
        }
        if status == StatusCode::FORBIDDEN {
            self.forbidden_total.fetch_add(1, Ordering::Relaxed);
        }

        let mut routes = self.route_requests.lock().await;
        *routes.entry(route.to_string()).or_insert(0) += 1;
        drop(routes);

        let mut statuses = self.status_counts.lock().await;
        *statuses.entry(status.as_u16()).or_insert(0) += 1;
    }

    fn record_runtime_update(&self) {
        self.runtime_updates_total.fetch_add(1, Ordering::Relaxed);
    }

    fn record_discovery_search(&self) {
        self.discovery_search_total.fetch_add(1, Ordering::Relaxed);
    }

    async fn snapshot(&self) -> AdminMetricsSnapshot {
        let route_requests = self.route_requests.lock().await.clone();
        let status_counts = self.status_counts.lock().await.clone();

        AdminMetricsSnapshot {
            requests_total: self.requests_total.load(Ordering::Relaxed),
            unauthorized_total: self.unauthorized_total.load(Ordering::Relaxed),
            forbidden_total: self.forbidden_total.load(Ordering::Relaxed),
            runtime_updates_total: self.runtime_updates_total.load(Ordering::Relaxed),
            discovery_search_total: self.discovery_search_total.load(Ordering::Relaxed),
            route_requests,
            status_counts: status_counts
                .into_iter()
                .map(|(status, count)| (status.to_string(), count))
                .collect(),
        }
    }
}

pub async fn spawn_admin_server(
    addr: SocketAddr,
    runtime: RuntimeArgsStore,
    handler: Arc<dyn Fn(RuntimeUpdate) -> BoxFuture<'static, RuntimeApplyResult> + Send + Sync>,
    options: AdminServerOptions,
) {
    let state = AdminState {
        runtime,
        handler,
        options,
        metrics: Arc::new(AdminMetrics::default()),
    };

    let router = build_router()
        .with_state(state.clone())
        .layer(middleware::from_fn_with_state(
            state.clone(),
            authorize_and_record_admin_request,
        ));

    tracing::info!("Runtime admin endpoint listening on http://{addr}");

    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(listener) => listener,
        Err(err) => {
            tracing::error!("Runtime admin bind error: {err}");
            return;
        }
    };
    let server = axum::serve(
        listener,
        router.into_make_service_with_connect_info::<SocketAddr>(),
    );
    if let Err(err) = server.await {
        tracing::error!("Runtime admin server error: {err}");
    }
}

fn build_router() -> Router<AdminState> {
    Router::new()
        .route("/v1/runtime/defaults", post(update_defaults))
        .route("/v1/runtime/session/{id}", post(update_session))
        .route("/v1/runtime/sessions", get(list_sessions))
        .route("/v1/runtime/health", get(runtime_health))
        .route("/v1/runtime/metrics", get(runtime_metrics_json))
        .route("/v1/runtime/metrics.prom", get(runtime_metrics_prometheus))
        .route("/v1/discovery/search", post(discovery_search))
}

async fn authorize_and_record_admin_request(
    State(state): State<AdminState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    req: axum::http::Request<Body>,
    next: Next,
) -> Response {
    let route = req
        .extensions()
        .get::<MatchedPath>()
        .map(|matched| matched.as_str().to_string())
        .unwrap_or_else(|| req.uri().path().to_string());

    let response = if state.options.loopback_only && !addr.ip().is_loopback() {
        json_error(
            StatusCode::FORBIDDEN,
            "runtime admin endpoint is loopback-only",
        )
    } else if let Some(expected) = state.options.bearer_token.as_deref() {
        if !matches_admin_token(req.headers(), expected) {
            json_error(StatusCode::UNAUTHORIZED, "missing or invalid admin token")
        } else {
            next.run(req).await
        }
    } else {
        next.run(req).await
    };

    state
        .metrics
        .record_request(&route, response.status())
        .await;
    response
}

fn matches_admin_token(headers: &HeaderMap, expected: &str) -> bool {
    if let Some(value) = headers.get(axum::http::header::AUTHORIZATION) {
        if let Ok(raw) = value.to_str() {
            if let Some(actual) = raw.strip_prefix("Bearer ") {
                if actual.trim() == expected {
                    return true;
                }
            }
        }
    }
    false
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

fn render_prometheus(snapshot: &AdminMetricsSnapshot) -> String {
    let mut lines = Vec::new();
    lines.push("# HELP mcpway_admin_requests_total Total admin API requests.".to_string());
    lines.push("# TYPE mcpway_admin_requests_total counter".to_string());
    lines.push(format!(
        "mcpway_admin_requests_total {}",
        snapshot.requests_total
    ));

    lines.push(
        "# HELP mcpway_admin_unauthorized_total Unauthorized admin API requests.".to_string(),
    );
    lines.push("# TYPE mcpway_admin_unauthorized_total counter".to_string());
    lines.push(format!(
        "mcpway_admin_unauthorized_total {}",
        snapshot.unauthorized_total
    ));

    lines.push("# HELP mcpway_admin_forbidden_total Forbidden admin API requests.".to_string());
    lines.push("# TYPE mcpway_admin_forbidden_total counter".to_string());
    lines.push(format!(
        "mcpway_admin_forbidden_total {}",
        snapshot.forbidden_total
    ));

    lines.push(
        "# HELP mcpway_admin_runtime_updates_total Runtime update requests accepted.".to_string(),
    );
    lines.push("# TYPE mcpway_admin_runtime_updates_total counter".to_string());
    lines.push(format!(
        "mcpway_admin_runtime_updates_total {}",
        snapshot.runtime_updates_total
    ));

    lines.push(
        "# HELP mcpway_admin_discovery_search_total Discovery search API requests.".to_string(),
    );
    lines.push("# TYPE mcpway_admin_discovery_search_total counter".to_string());
    lines.push(format!(
        "mcpway_admin_discovery_search_total {}",
        snapshot.discovery_search_total
    ));

    lines.push("# HELP mcpway_admin_route_requests_total Requests per admin route.".to_string());
    lines.push("# TYPE mcpway_admin_route_requests_total counter".to_string());
    for (route, count) in &snapshot.route_requests {
        lines.push(format!(
            "mcpway_admin_route_requests_total{{route=\"{}\"}} {}",
            prometheus_escape(route),
            count
        ));
    }

    lines.push("# HELP mcpway_admin_status_total Requests by HTTP status.".to_string());
    lines.push("# TYPE mcpway_admin_status_total counter".to_string());
    for (status, count) in &snapshot.status_counts {
        lines.push(format!(
            "mcpway_admin_status_total{{status=\"{}\"}} {}",
            prometheus_escape(status),
            count
        ));
    }

    lines.join("\n") + "\n"
}

fn prometheus_escape(input: &str) -> String {
    input.replace('\\', "\\\\").replace('"', "\\\"")
}

#[derive(Debug, Deserialize, Default)]
struct DiscoverySearchRequest {
    #[serde(default)]
    from: Option<SourceKind>,
    #[serde(default)]
    project_root: Option<String>,
    #[serde(flatten, default)]
    search: DiscoverySearchOptions,
}

async fn update_defaults(
    State(state): State<AdminState>,
    Json(update): Json<RuntimeArgsUpdate>,
) -> impl IntoResponse {
    state.metrics.record_runtime_update();
    let update_msg = RuntimeUpdate {
        scope: RuntimeScope::Global,
        update,
    };
    Json((state.handler)(update_msg).await)
}

async fn update_session(
    State(state): State<AdminState>,
    Path(id): Path<String>,
    Json(update): Json<RuntimeArgsUpdate>,
) -> impl IntoResponse {
    state.metrics.record_runtime_update();
    let update_msg = RuntimeUpdate {
        scope: RuntimeScope::Session(id),
        update,
    };
    Json((state.handler)(update_msg).await)
}

async fn list_sessions(State(state): State<AdminState>) -> impl IntoResponse {
    let sessions = state.runtime.list_sessions().await;
    Json(sessions)
}

async fn runtime_health(State(state): State<AdminState>) -> impl IntoResponse {
    let sessions = state.runtime.list_sessions().await;
    Json(serde_json::json!({
        "status": "ok",
        "sessions": sessions.len(),
        "loopback_only": state.options.loopback_only,
        "auth_enabled": state.options.bearer_token.is_some(),
    }))
}

async fn runtime_metrics_json(State(state): State<AdminState>) -> impl IntoResponse {
    Json(state.metrics.snapshot().await)
}

async fn runtime_metrics_prometheus(State(state): State<AdminState>) -> impl IntoResponse {
    let snapshot = state.metrics.snapshot().await;
    (
        StatusCode::OK,
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        render_prometheus(&snapshot),
    )
}

async fn discovery_search(
    State(state): State<AdminState>,
    Json(request): Json<DiscoverySearchRequest>,
) -> impl IntoResponse {
    state.metrics.record_discovery_search();
    let discover_options = DiscoverOptions {
        from: request.from.or(state.options.discovery_source),
        project_root: request
            .project_root
            .as_deref()
            .map(PathBuf::from)
            .or_else(|| state.options.discovery_project_root.clone()),
    };

    let report = match discovery::discover(&discover_options) {
        Ok(report) => report,
        Err(err) => return json_error(StatusCode::BAD_REQUEST, &err),
    };
    let total = report.servers.len();
    let filtered = discovery::apply_search(&report, &request.search);

    Json(serde_json::json!({
        "total": total,
        "returned": filtered.servers.len(),
        "report": filtered,
    }))
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn matches_admin_token_accepts_bearer_header() {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            "Bearer secret-token".parse().expect("valid header"),
        );
        assert!(matches_admin_token(&headers, "secret-token"));
    }

    #[test]
    fn matches_admin_token_rejects_legacy_header() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-mcpway-token",
            "legacy-token".parse().expect("valid header"),
        );
        assert!(!matches_admin_token(&headers, "legacy-token"));
    }

    #[tokio::test]
    async fn metrics_snapshot_tracks_route_and_status_counts() {
        let metrics = AdminMetrics::default();
        metrics
            .record_request("/v1/runtime/health", StatusCode::OK)
            .await;
        metrics
            .record_request("/v1/runtime/health", StatusCode::UNAUTHORIZED)
            .await;
        metrics.record_runtime_update();
        metrics.record_discovery_search();

        let snapshot = metrics.snapshot().await;
        assert_eq!(snapshot.requests_total, 2);
        assert_eq!(snapshot.unauthorized_total, 1);
        assert_eq!(snapshot.runtime_updates_total, 1);
        assert_eq!(snapshot.discovery_search_total, 1);
        assert_eq!(snapshot.route_requests["/v1/runtime/health"], 2);
        assert_eq!(snapshot.status_counts["200"], 1);
        assert_eq!(snapshot.status_counts["401"], 1);
    }

    #[test]
    fn prometheus_render_includes_core_counters() {
        let snapshot = AdminMetricsSnapshot {
            requests_total: 5,
            unauthorized_total: 1,
            forbidden_total: 1,
            runtime_updates_total: 2,
            discovery_search_total: 3,
            route_requests: BTreeMap::from([(String::from("/v1/runtime/health"), 4)]),
            status_counts: BTreeMap::from([(String::from("200"), 4)]),
        };

        let rendered = render_prometheus(&snapshot);
        assert!(rendered.contains("mcpway_admin_requests_total 5"));
        assert!(rendered.contains("mcpway_admin_runtime_updates_total 2"));
        assert!(rendered.contains("mcpway_admin_discovery_search_total 3"));
        assert!(rendered.contains("route=\"/v1/runtime/health\""));
    }

    #[tokio::test]
    async fn legacy_runtime_routes_are_removed() {
        let runtime = RuntimeArgsStore::default();
        let handler: Arc<
            dyn Fn(RuntimeUpdate) -> BoxFuture<'static, RuntimeApplyResult> + Send + Sync,
        > = Arc::new(|_update: RuntimeUpdate| {
            Box::pin(async { RuntimeApplyResult::ok("ok", false) })
        });
        let state = AdminState {
            runtime,
            handler,
            options: AdminServerOptions::default(),
            metrics: Arc::new(AdminMetrics::default()),
        };

        let app = build_router()
            .with_state(state.clone())
            .layer(middleware::from_fn_with_state(
                state,
                authorize_and_record_admin_request,
            ));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind listener");
        let addr = listener.local_addr().expect("listener addr");
        tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .expect("serve admin test app");
        });

        let client = reqwest::Client::new();
        let legacy = get_with_retry(&client, &format!("http://{addr}/runtime/sessions")).await;
        assert_eq!(legacy.status(), StatusCode::NOT_FOUND);

        let v1 = get_with_retry(&client, &format!("http://{addr}/v1/runtime/sessions")).await;
        assert_eq!(v1.status(), StatusCode::OK);
    }

    async fn get_with_retry(client: &reqwest::Client, url: &str) -> reqwest::Response {
        let mut last_error = None;
        for _ in 0..20u8 {
            match client.get(url).send().await {
                Ok(response) => return response,
                Err(err) => {
                    last_error = Some(err);
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            }
        }
        panic!(
            "request did not succeed for {url}: {}",
            last_error
                .map(|err| err.to_string())
                .unwrap_or_else(|| "unknown error".to_string())
        );
    }
}
