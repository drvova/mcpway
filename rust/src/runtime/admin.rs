use std::net::SocketAddr;
use std::sync::Arc;

use crate::runtime::store::{RuntimeArgsStore, RuntimeArgsUpdate};
use crate::runtime::{RuntimeApplyResult, RuntimeScope, RuntimeUpdate};
use axum::extract::{ConnectInfo, Path, State};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures::future::BoxFuture;

#[derive(Clone)]
pub struct AdminState {
    runtime: RuntimeArgsStore,
    handler: Arc<dyn Fn(RuntimeUpdate) -> BoxFuture<'static, RuntimeApplyResult> + Send + Sync>,
}

pub async fn spawn_admin_server(
    addr: SocketAddr,
    runtime: RuntimeArgsStore,
    handler: Arc<dyn Fn(RuntimeUpdate) -> BoxFuture<'static, RuntimeApplyResult> + Send + Sync>,
) {
    let state = AdminState { runtime, handler };

    let router = Router::new()
        .route("/runtime/defaults", post(update_defaults))
        .route("/runtime/session/:id", post(update_session))
        .route("/runtime/sessions", get(list_sessions))
        .with_state(state)
        .layer(middleware::from_fn(only_loopback));

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

async fn only_loopback(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    req: axum::http::Request<axum::body::Body>,
    next: Next,
) -> Response {
    if !addr.ip().is_loopback() {
        return StatusCode::FORBIDDEN.into_response();
    }
    next.run(req).await
}

async fn update_defaults(
    State(state): State<AdminState>,
    Json(update): Json<RuntimeArgsUpdate>,
) -> impl IntoResponse {
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
