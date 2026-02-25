use std::sync::Arc;
use std::time::Duration;

use eventsource_stream::Eventsource;
use futures::StreamExt;
use tokio::sync::{mpsc, RwLock};
use tokio_util::codec::{FramedRead, LinesCodec};
use uuid::Uuid;

use crate::config::Config;
use crate::runtime::store::RuntimeArgsStore;
use crate::runtime::{RuntimeApplyResult, RuntimeScope, RuntimeUpdateRequest};
use crate::support::signals::install_signal_handlers;
use crate::transport::pool::{global_pool, transport_fingerprint, TransportPool};
use crate::types::HeadersMap;

const HTTP_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const HTTP_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

pub async fn run(
    config: Config,
    runtime: RuntimeArgsStore,
    mut updates: mpsc::Receiver<RuntimeUpdateRequest>,
) -> Result<(), String> {
    let streamable_http_url = config
        .streamable_http
        .clone()
        .ok_or("streamableHttp url is required")?;
    tracing::info!("  - streamableHttp: {streamable_http_url}");
    tracing::info!(
        "  - Headers: {}",
        serde_json::to_string(&config.headers).unwrap_or_else(|_| "(none)".into())
    );
    tracing::info!("Connecting to Streamable HTTP...");

    install_signal_handlers(None);

    let session_id: Arc<RwLock<Option<String>>> = Arc::new(RwLock::new(None));
    let session_for_sse = session_id.clone();
    let headers = config.headers.clone();
    let protocol_version = config.protocol_version.clone();
    let pool = global_pool();
    let sse_key = transport_fingerprint(
        "streamable-http-sse",
        &streamable_http_url,
        &headers,
        &protocol_version,
    );
    let request_key = transport_fingerprint(
        "streamable-http-request",
        &streamable_http_url,
        &headers,
        &protocol_version,
    );
    let sse_http = pool
        .http_client(&sse_key, HTTP_CONNECT_TIMEOUT, None)
        .await?;
    let http = pool
        .http_client(
            &request_key,
            HTTP_CONNECT_TIMEOUT,
            Some(HTTP_REQUEST_TIMEOUT),
        )
        .await?;
    let session_clone = session_id.clone();
    let runtime_clone = runtime.clone();
    let headers_clone = headers.clone();
    let sse_http_clone = sse_http.clone();
    let url_clone = streamable_http_url.clone();
    let sse_pool = pool.clone();
    let sse_key_clone = sse_key.clone();
    tokio::spawn(async move {
        loop {
            let Some(sid) = session_for_sse.read().await.clone() else {
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                continue;
            };
            let mut req = sse_http_clone
                .get(&url_clone)
                .header("Accept", "text/event-stream");
            for (k, v) in headers_clone
                .iter()
                .chain(runtime_clone.get_effective(None).await.headers.iter())
            {
                req = req.header(k, v);
            }
            req = req.header("Mcp-Session-Id", sid.clone());
            let response = match req.send().await {
                Ok(resp) => resp,
                Err(err) => {
                    tracing::error!("Streamable HTTP SSE connection failed: {err}");
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    continue;
                }
            };
            sse_pool
                .mark_success(&sse_key_clone, "streamable-http")
                .await;
            let stream = response.bytes_stream().eventsource();
            tokio::pin!(stream);
            while let Some(event) = stream.next().await {
                match event {
                    Ok(event) => {
                        if event.data.trim().is_empty() {
                            continue;
                        }
                        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&event.data) {
                            println!("{}", json);
                        }
                    }
                    Err(err) => {
                        tracing::error!("Streamable HTTP SSE error: {err}");
                        break;
                    }
                }
            }
        }
    });

    let runtime_store = runtime.clone();
    tokio::spawn(async move {
        while let Some(req) = updates.recv().await {
            let result = match req.update.scope {
                RuntimeScope::Global => {
                    let update_result = runtime_store.update_global(req.update.update).await;
                    if update_result.restart_needed {
                        RuntimeApplyResult::ok(
                            "Updated runtime args; env/CLI changes require restart of remote server",
                            false,
                        )
                    } else {
                        RuntimeApplyResult::ok("Updated runtime headers", false)
                    }
                }
                RuntimeScope::Session(_) => RuntimeApplyResult::error(
                    "Per-session runtime overrides are not supported for StreamableHTTP→stdio",
                ),
            };
            let _ = req.respond_to.send(result);
        }
    });

    let mut lines = FramedRead::new(tokio::io::stdin(), LinesCodec::new());
    let mut initialized = false;

    while let Some(line) = lines.next().await {
        let line = line.map_err(|err| err.to_string())?;
        if line.trim().is_empty() {
            continue;
        }
        let Ok(message) = serde_json::from_str::<serde_json::Value>(&line) else {
            tracing::error!("Invalid JSON from stdin: {line}");
            continue;
        };

        if !is_request(&message) {
            println!("{}", message);
            continue;
        }

        let runtime_args = runtime.get_effective(None).await;
        if !initialized && !is_initialize_request(&message) {
            let init_id = auto_init_id();
            let init_message = create_initialize_request(&init_id, &protocol_version);
            let init_payload = send_request(
                &http,
                &streamable_http_url,
                &runtime_args.headers,
                &session_clone,
                &pool,
                &request_key,
                &init_message,
            )
            .await;
            if init_payload.get("error").is_some() {
                let response = wrap_response(&message, init_payload);
                println!("{}", response);
                continue;
            }
            if let Err(err) = send_initialized_notification(
                &http,
                &streamable_http_url,
                &runtime_args.headers,
                &session_clone,
                &pool,
                &request_key,
            )
            .await
            {
                tracing::error!("Failed to send initialized notification: {err}");
            } else {
                initialized = true;
            }
        }

        let payload = send_request(
            &http,
            &streamable_http_url,
            &runtime_args.headers,
            &session_clone,
            &pool,
            &request_key,
            &message,
        )
        .await;

        if is_initialize_request(&message) && payload.get("error").is_none() && !initialized {
            if let Err(err) = send_initialized_notification(
                &http,
                &streamable_http_url,
                &runtime_args.headers,
                &session_clone,
                &pool,
                &request_key,
            )
            .await
            {
                tracing::error!("Failed to send initialized notification: {err}");
            } else {
                initialized = true;
            }
        }

        let response = wrap_response(&message, payload);
        println!("{}", response);
    }

    Ok(())
}

fn is_request(message: &serde_json::Value) -> bool {
    message.get("method").is_some() && message.get("id").is_some()
}

fn is_initialize_request(message: &serde_json::Value) -> bool {
    message
        .get("method")
        .and_then(|method| method.as_str())
        .map(|method| method == "initialize")
        .unwrap_or(false)
}

fn auto_init_id() -> String {
    format!(
        "init_{}_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
        Uuid::new_v4()
    )
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

async fn send_request(
    http: &reqwest::Client,
    url: &str,
    headers: &HeadersMap,
    session_id: &Arc<RwLock<Option<String>>>,
    pool: &Arc<TransportPool>,
    pool_key: &str,
    message: &serde_json::Value,
) -> serde_json::Value {
    let mut req = http.post(url).json(message);
    for (k, v) in headers.iter() {
        req = req.header(k, v);
    }
    if let Some(sid) = session_id.read().await.clone() {
        req = req.header("Mcp-Session-Id", sid);
    }
    match req.send().await {
        Ok(resp) => {
            pool.mark_success(pool_key, "streamable-http").await;
            if let Some(sid) = resp
                .headers()
                .get("Mcp-Session-Id")
                .and_then(|v| v.to_str().ok())
            {
                *session_id.write().await = Some(sid.to_string());
            }
            match parse_response_payload(resp).await {
                Ok(payload) => payload,
                Err(err) => error_payload(-32000, err),
            }
        }
        Err(err) => error_payload(-32000, err.to_string()),
    }
}

async fn send_initialized_notification(
    http: &reqwest::Client,
    url: &str,
    headers: &HeadersMap,
    session_id: &Arc<RwLock<Option<String>>>,
    pool: &Arc<TransportPool>,
    pool_key: &str,
) -> Result<(), String> {
    let message = create_initialized_notification();
    let mut req = http.post(url).json(&message);
    for (k, v) in headers.iter() {
        req = req.header(k, v);
    }
    if let Some(sid) = session_id.read().await.clone() {
        req = req.header("Mcp-Session-Id", sid);
    }
    let response = req.send().await.map_err(|err| err.to_string())?;
    pool.mark_success(pool_key, "streamable-http").await;
    if let Some(sid) = response
        .headers()
        .get("Mcp-Session-Id")
        .and_then(|v| v.to_str().ok())
    {
        *session_id.write().await = Some(sid.to_string());
    }
    if response.status().is_success() {
        Ok(())
    } else {
        Err(format!(
            "Initialized notification failed with status {}",
            response.status()
        ))
    }
}

async fn parse_response_payload(resp: reqwest::Response) -> Result<serde_json::Value, String> {
    let status = resp.status();
    let text = resp.text().await.map_err(|err| err.to_string())?;
    if text.trim().is_empty() {
        if status.is_success() {
            return Err("Empty response".to_string());
        }
        return Err(format!("Request failed with status {}", status));
    }
    let json: serde_json::Value = serde_json::from_str(&text).map_err(|err| err.to_string())?;
    if !status.is_success() {
        if let Some(error) = json.get("error") {
            return Ok(serde_json::json!({ "error": error }));
        }
        return Err(format!("Request failed with status {}", status));
    }
    if json.get("error").is_some() {
        return Ok(serde_json::json!({ "error": json.get("error").cloned().unwrap_or_default() }));
    }
    if let Some(result) = json.get("result") {
        return Ok(serde_json::json!({ "result": result }));
    }
    Ok(serde_json::json!({ "result": json }))
}

fn wrap_response(req: &serde_json::Value, payload: serde_json::Value) -> serde_json::Value {
    let jsonrpc = req
        .get("jsonrpc")
        .cloned()
        .unwrap_or_else(|| serde_json::Value::String("2.0".to_string()));
    let id = req.get("id").cloned().unwrap_or(serde_json::Value::Null);

    let mut response = serde_json::Map::new();
    response.insert("jsonrpc".to_string(), jsonrpc);
    response.insert("id".to_string(), id);

    if let Some(error) = payload.get("error") {
        if let Some(code) = error.get("code").and_then(|v| v.as_i64()) {
            let message = error
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("Internal error");
            response.insert(
                "error".to_string(),
                serde_json::json!({
                    "code": code,
                    "message": normalize_error_message(code, message),
                }),
            );
        } else {
            response.insert("error".to_string(), error.clone());
        }
    } else if let Some(result) = payload.get("result") {
        response.insert("result".to_string(), result.clone());
    }

    serde_json::Value::Object(response)
}

fn error_payload(code: i64, message: impl Into<String>) -> serde_json::Value {
    serde_json::json!({
        "error": {
            "code": code,
            "message": message.into(),
        }
    })
}

fn normalize_error_message(code: i64, message: &str) -> String {
    let prefix = format!("MCP error {code}:");
    if message.starts_with(&prefix) {
        message[prefix.len()..].trim().to_string()
    } else {
        message.to_string()
    }
}
