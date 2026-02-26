use std::sync::Arc;
use std::time::{Duration, Instant};

use eventsource_stream::Eventsource;
use futures::StreamExt;
use reqwest::Url;
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

const ENDPOINT_WAIT_TIMEOUT: Duration = Duration::from_secs(10);
const HTTP_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const HTTP_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

pub async fn run(
    config: Config,
    runtime: RuntimeArgsStore,
    mut updates: mpsc::Receiver<RuntimeUpdateRequest>,
) -> Result<(), String> {
    let sse_url = config.sse.clone().ok_or("sse url is required")?;
    tracing::info!("  - sse: {sse_url}");
    tracing::info!(
        "  - Headers: {}",
        serde_json::to_string(&config.headers).unwrap_or_else(|_| "(none)".into())
    );
    tracing::info!("Connecting to SSE...");

    install_signal_handlers(None);

    let message_endpoint: Arc<RwLock<Option<Url>>> = Arc::new(RwLock::new(None));
    let headers = config.headers.clone();
    let protocol_version = config.protocol_version.clone();
    let sse_url_clone = sse_url.clone();
    let message_endpoint_clone = message_endpoint.clone();
    let runtime_clone = runtime.clone();
    let pool = global_pool();
    let sse_key = transport_fingerprint("sse-events", &sse_url, &headers, &protocol_version);
    let request_key = transport_fingerprint("sse-request", &sse_url, &headers, &protocol_version);
    let sse_pool = pool.clone();
    let sse_key_clone = sse_key.clone();

    tokio::spawn(async move {
        let client = match sse_pool
            .http_client(&sse_key_clone, HTTP_CONNECT_TIMEOUT, None)
            .await
        {
            Ok(client) => client,
            Err(err) => {
                tracing::error!("Failed to create SSE client: {err}");
                return;
            }
        };
        let mut req = client.get(&sse_url_clone);
        for (k, v) in &headers {
            req = req.header(k, v);
        }
        let response = match req.send().await {
            Ok(resp) => resp,
            Err(err) => {
                tracing::error!("SSE connection failed: {err}");
                return;
            }
        };
        sse_pool.mark_success(&sse_key_clone, "sse").await;
        let stream = response.bytes_stream().eventsource();
        tokio::pin!(stream);
        while let Some(event) = stream.next().await {
            match event {
                Ok(event) => {
                    if event.event == "endpoint" {
                        if let Ok(url) = Url::parse(&sse_url_clone) {
                            if let Ok(joined) = url.join(&event.data) {
                                *message_endpoint_clone.write().await = Some(joined.clone());
                                tracing::info!("Received message endpoint: {joined}");
                            }
                        }
                        continue;
                    }
                    if event.data.trim().is_empty() {
                        continue;
                    }
                    if let Ok(json) = serde_json::from_str::<serde_json::Value>(&event.data) {
                        println!("{}", json);
                    }
                }
                Err(err) => {
                    tracing::error!("SSE error: {err}");
                    break;
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
                    "Per-session runtime overrides are not supported for SSE→stdio",
                ),
            };
            let _ = req.respond_to.send(result);
        }
    });

    let stdin = tokio::io::stdin();
    let mut lines = FramedRead::new(stdin, LinesCodec::new());
    let http = pool
        .http_client(
            &request_key,
            HTTP_CONNECT_TIMEOUT,
            Some(HTTP_REQUEST_TIMEOUT),
        )
        .await?;
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

        let endpoint = match wait_for_message_endpoint(&message_endpoint).await {
            Ok(endpoint) => endpoint,
            Err(err) => {
                let response = wrap_response(&message, error_payload(-32000, err));
                println!("{}", response);
                continue;
            }
        };

        let runtime_args = runtime_clone.get_effective(None).await;
        if !initialized && !is_initialize_request(&message) {
            let init_id = auto_init_id();
            let init_message = create_initialize_request(&init_id, &protocol_version);
            let init_payload = send_request(
                &http,
                &endpoint,
                &runtime_args.headers,
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
                &endpoint,
                &runtime_args.headers,
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
            &endpoint,
            &runtime_args.headers,
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
                &endpoint,
                &runtime_args.headers,
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

async fn wait_for_message_endpoint(
    message_endpoint: &Arc<RwLock<Option<Url>>>,
) -> Result<Url, String> {
    let deadline = Instant::now() + ENDPOINT_WAIT_TIMEOUT;
    loop {
        if let Some(url) = message_endpoint.read().await.clone() {
            return Ok(url);
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "Timed out waiting for SSE endpoint after {}ms",
                ENDPOINT_WAIT_TIMEOUT.as_millis()
            ));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
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
    endpoint: &Url,
    headers: &HeadersMap,
    pool: &Arc<TransportPool>,
    pool_key: &str,
    message: &serde_json::Value,
    retry_policy: RetryPolicy,
    circuit_breaker: &mut CircuitBreaker,
) -> serde_json::Value {
    match run_with_retry("sse-request", retry_policy, circuit_breaker, || async {
        let mut req = http.post(endpoint.clone()).json(message);
        for (k, v) in headers.iter() {
            req = req.header(k, v);
        }
        let resp = req.send().await.map_err(|err| err.to_string())?;
        pool.mark_success(pool_key, "sse").await;
        parse_response_payload(resp).await
    })
    .await
    {
        Ok(payload) => payload,
        Err(err) => error_payload(-32000, err),
    }
}

async fn send_initialized_notification(
    http: &reqwest::Client,
    endpoint: &Url,
    headers: &HeadersMap,
    pool: &Arc<TransportPool>,
    pool_key: &str,
    retry_policy: RetryPolicy,
    circuit_breaker: &mut CircuitBreaker,
) -> Result<(), String> {
    run_with_retry(
        "sse-initialized-notification",
        retry_policy,
        circuit_breaker,
        || async {
            let message = create_initialized_notification();
            let mut req = http.post(endpoint.clone()).json(&message);
            for (k, v) in headers.iter() {
                req = req.header(k, v);
            }
            let response = req.send().await.map_err(|err| err.to_string())?;
            pool.mark_success(pool_key, "sse").await;
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
