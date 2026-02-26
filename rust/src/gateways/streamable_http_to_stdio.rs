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
use crate::transport::reliability::{
    run_with_retry, CircuitBreaker, CircuitBreakerPolicy, RetryPolicy,
};
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
        .ok_or("streamable-http url is required")?;
    tracing::info!("  - streamable-http: {streamable_http_url}");
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
    let retry_policy = RetryPolicy {
        max_retries: config.retry_attempts,
        base_delay: Duration::from_millis(config.retry_base_delay_ms),
        max_delay: Duration::from_millis(config.retry_max_delay_ms),
    };
    let mut circuit_breaker = CircuitBreaker::new(CircuitBreakerPolicy {
        failure_threshold: config.circuit_failure_threshold,
        cooldown: Duration::from_millis(config.circuit_cooldown_ms),
    });
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
                retry_policy,
                &mut circuit_breaker,
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
                retry_policy,
                &mut circuit_breaker,
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
            retry_policy,
            &mut circuit_breaker,
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
                retry_policy,
                &mut circuit_breaker,
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
    retry_policy: RetryPolicy,
    circuit_breaker: &mut CircuitBreaker,
) -> serde_json::Value {
    match run_with_retry(
        "streamable-http-request",
        retry_policy,
        circuit_breaker,
        || async {
            let mut req = apply_request_headers(http.post(url).json(message), headers);
            if let Some(sid) = session_id.read().await.clone() {
                req = req.header("Mcp-Session-Id", sid);
            }
            let resp = req.send().await.map_err(|err| err.to_string())?;
            pool.mark_success(pool_key, "streamable-http").await;
            if let Some(sid) = extract_session_id(resp.headers()) {
                *session_id.write().await = Some(sid.to_string());
            }
            parse_response_payload(resp).await
        },
    )
    .await
    {
        Ok(payload) => payload,
        Err(err) => error_payload(-32000, err),
    }
}

async fn send_initialized_notification(
    http: &reqwest::Client,
    url: &str,
    headers: &HeadersMap,
    session_id: &Arc<RwLock<Option<String>>>,
    pool: &Arc<TransportPool>,
    pool_key: &str,
    retry_policy: RetryPolicy,
    circuit_breaker: &mut CircuitBreaker,
) -> Result<(), String> {
    run_with_retry(
        "streamable-http-initialized-notification",
        retry_policy,
        circuit_breaker,
        || async {
            let message = create_initialized_notification();
            let mut req = apply_request_headers(http.post(url).json(&message), headers);
            if let Some(sid) = session_id.read().await.clone() {
                req = req.header("Mcp-Session-Id", sid);
            }
            let response = req.send().await.map_err(|err| err.to_string())?;
            pool.mark_success(pool_key, "streamable-http").await;
            if let Some(sid) = extract_session_id(response.headers()) {
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
        },
    )
    .await
}

async fn parse_response_payload(resp: reqwest::Response) -> Result<serde_json::Value, String> {
    let status = resp.status();
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.to_lowercase())
        .unwrap_or_default();
    let text = resp.text().await.map_err(|err| err.to_string())?;
    if text.trim().is_empty() {
        if status.is_success() {
            return Err("Empty response".to_string());
        }
        return Err(format!("Request failed with status {}", status));
    }
    let json = parse_response_body(&content_type, &text)?;
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

fn apply_request_headers(
    mut req: reqwest::RequestBuilder,
    headers: &HeadersMap,
) -> reqwest::RequestBuilder {
    let mut has_accept = false;
    for (key, value) in headers {
        if key.eq_ignore_ascii_case("accept") {
            has_accept = true;
        }
        req = req.header(key, value);
    }
    if !has_accept {
        req = req.header(
            reqwest::header::ACCEPT,
            "application/json, text/event-stream",
        );
    }
    req
}

fn extract_session_id(headers: &reqwest::header::HeaderMap) -> Option<String> {
    headers
        .get("Mcp-Session-Id")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
}

fn parse_response_body(content_type: &str, body: &str) -> Result<serde_json::Value, String> {
    if content_type.contains("text/event-stream") {
        return parse_event_stream_body(body);
    }
    serde_json::from_str(body).map_err(|err| err.to_string())
}

fn parse_event_stream_body(body: &str) -> Result<serde_json::Value, String> {
    let mut data_lines: Vec<String> = Vec::new();
    for raw in body.lines() {
        let line = raw.trim_end_matches('\r');
        if line.is_empty() {
            if let Some(json) = parse_sse_event_data(&data_lines) {
                return Ok(json);
            }
            data_lines.clear();
            continue;
        }
        if let Some(data) = line.strip_prefix("data:") {
            data_lines.push(data.trim_start().to_string());
        }
    }
    if let Some(json) = parse_sse_event_data(&data_lines) {
        return Ok(json);
    }
    serde_json::from_str(body).map_err(|_| "No JSON payload found in event-stream response".into())
}

fn parse_sse_event_data(data_lines: &[String]) -> Option<serde_json::Value> {
    if data_lines.is_empty() {
        return None;
    }
    let payload = data_lines.join("\n");
    serde_json::from_str(&payload).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_json_response_body() {
        let value = parse_response_body("application/json", "{\"result\":{\"ok\":true}}")
            .expect("application/json body should parse");
        assert_eq!(value["result"]["ok"], serde_json::json!(true));
    }

    #[test]
    fn parses_sse_framed_json_response_body() {
        let value = parse_response_body(
            "text/event-stream",
            "event: message\ndata: {\"result\":{\"ok\":true},\"id\":1,\"jsonrpc\":\"2.0\"}\n\n",
        )
        .expect("text/event-stream body should parse first data payload");
        assert_eq!(value["result"]["ok"], serde_json::json!(true));
    }

    #[test]
    fn extracts_session_id_from_canonical_header() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::HeaderName::from_static("mcp-session-id"),
            reqwest::header::HeaderValue::from_static("session-123"),
        );
        assert_eq!(extract_session_id(&headers).as_deref(), Some("session-123"));
    }

    #[test]
    fn apply_request_headers_sets_accept_when_missing() {
        let client = reqwest::Client::new();
        let headers = HeadersMap::new();
        let request = apply_request_headers(
            client
                .post("http://127.0.0.1:9/mcp")
                .json(&serde_json::json!({"ping": true})),
            &headers,
        )
        .build()
        .expect("request should build");
        let accept = request
            .headers()
            .get(reqwest::header::ACCEPT)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        assert_eq!(accept, "application/json, text/event-stream");
    }
}
