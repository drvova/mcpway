use std::sync::Arc;
use std::time::{Duration, Instant};

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
const SSE_WAIT_FOR_SESSION_DELAY: Duration = Duration::from_millis(200);
const SSE_RETRY_INITIAL_DELAY: Duration = Duration::from_millis(500);
const SSE_RETRY_MAX_DELAY: Duration = Duration::from_secs(30);
const SSE_RETRY_MULTIPLIER: u32 = 2;
const SSE_RETRY_JITTER_BPS: u16 = 2_000;
const SSE_ERROR_SUPPRESSION_WINDOW: Duration = Duration::from_secs(30);
const SSE_ERROR_SNIPPET_LIMIT: usize = 240;
const MCP_INTERNAL_ERROR_CODE: i64 = -32000;

#[derive(Debug, Clone)]
struct SseReconnectBackoff {
    initial_delay: Duration,
    current_delay: Duration,
    max_delay: Duration,
    multiplier: u32,
    jitter_bps: u16,
}

impl SseReconnectBackoff {
    fn new(initial_delay: Duration, max_delay: Duration, multiplier: u32, jitter_bps: u16) -> Self {
        Self {
            initial_delay,
            current_delay: initial_delay,
            max_delay,
            multiplier: multiplier.max(1),
            jitter_bps,
        }
    }

    fn next_delay(&mut self) -> Duration {
        let delay = jittered_duration(self.current_delay, self.jitter_bps, jitter_seed());
        let Some(next) = self.current_delay.checked_mul(self.multiplier) else {
            self.current_delay = self.max_delay;
            return delay;
        };
        self.current_delay = std::cmp::min(next, self.max_delay);
        delay
    }

    #[cfg(test)]
    fn next_delay_with_seed(&mut self, seed: u64) -> Duration {
        let delay = jittered_duration(self.current_delay, self.jitter_bps, seed);
        let Some(next) = self.current_delay.checked_mul(self.multiplier) else {
            self.current_delay = self.max_delay;
            return delay;
        };
        self.current_delay = std::cmp::min(next, self.max_delay);
        delay
    }

    fn reset(&mut self) {
        self.current_delay = self.initial_delay;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SseFailureCategory {
    Connect,
    HttpStatus,
    UnexpectedContentType,
    ProviderBusinessError,
    Stream,
    StreamEnded,
    NonJsonEvent,
}

impl SseFailureCategory {
    fn as_str(self) -> &'static str {
        match self {
            Self::Connect => "connect_error",
            Self::HttpStatus => "http_error_status",
            Self::UnexpectedContentType => "unexpected_content_type",
            Self::ProviderBusinessError => "provider_business_error",
            Self::Stream => "stream_error",
            Self::StreamEnded => "stream_ended",
            Self::NonJsonEvent => "non_json_event",
        }
    }
}

#[derive(Debug, Clone)]
struct SseFailureReport {
    category: SseFailureCategory,
    status: Option<u16>,
    message: String,
    auth_related: bool,
}

impl SseFailureReport {
    fn fingerprint(&self) -> String {
        format!(
            "{}|{}|{}",
            self.category.as_str(),
            self.status.unwrap_or(0),
            self.message
        )
    }
}

#[derive(Debug, Clone)]
enum SseLogDecision {
    Emit {
        suppressed: u64,
        previous_fingerprint: Option<String>,
    },
    Suppress,
}

#[derive(Debug, Default, Clone)]
struct SseErrorLogGate {
    last_fingerprint: Option<String>,
    last_log_at: Option<Instant>,
    suppressed_count: u64,
    auth_hint_logged: bool,
}

impl SseErrorLogGate {
    fn decide(&mut self, fingerprint: &str, now: Instant) -> SseLogDecision {
        if let (Some(last_fingerprint), Some(last_log_at)) =
            (self.last_fingerprint.as_deref(), self.last_log_at)
        {
            if last_fingerprint == fingerprint
                && now.duration_since(last_log_at) < SSE_ERROR_SUPPRESSION_WINDOW
            {
                self.suppressed_count = self.suppressed_count.saturating_add(1);
                return SseLogDecision::Suppress;
            }
        }

        let suppressed = self.suppressed_count;
        let previous_fingerprint = self.last_fingerprint.clone();
        self.last_fingerprint = Some(fingerprint.to_string());
        self.last_log_at = Some(now);
        self.suppressed_count = 0;
        SseLogDecision::Emit {
            suppressed,
            previous_fingerprint,
        }
    }

    fn flush_suppressed(&mut self) {
        if self.suppressed_count > 0 {
            tracing::warn!(
                target: "mcpway::gateways::streamable_http_to_stdio",
                suppressed = self.suppressed_count,
                "Suppressed repeated Streamable HTTP SSE errors"
            );
        }
        self.suppressed_count = 0;
        self.last_fingerprint = None;
        self.last_log_at = None;
    }

    fn log_failure(&mut self, report: SseFailureReport) {
        let fingerprint = report.fingerprint();
        match self.decide(&fingerprint, Instant::now()) {
            SseLogDecision::Suppress => return,
            SseLogDecision::Emit {
                suppressed,
                previous_fingerprint,
            } => {
                if suppressed > 0 {
                    tracing::warn!(
                        target: "mcpway::gateways::streamable_http_to_stdio",
                        suppressed,
                        previous = previous_fingerprint.unwrap_or_default(),
                        "Suppressed repeated Streamable HTTP SSE errors"
                    );
                }
            }
        }

        tracing::error!(
            target: "mcpway::gateways::streamable_http_to_stdio",
            category = report.category.as_str(),
            status = report.status.unwrap_or(0),
            message = report.message,
            "Streamable HTTP SSE connection failed"
        );

        if report.auth_related && !self.auth_hint_logged {
            self.auth_hint_logged = true;
            tracing::warn!(
                target: "mcpway::gateways::streamable_http_to_stdio",
                "Auth may be missing or malformed. Use --header \"Authorization: Bearer <token>\" or --oauth2-bearer <token>."
            );
        }
    }
}

fn classify_sse_handshake_failure(
    status: reqwest::StatusCode,
    content_type: &str,
    body: &str,
) -> SseFailureReport {
    let status_code = Some(status.as_u16());
    if !status.is_success() {
        let message = if body.trim().is_empty() {
            format!("HTTP {status} from SSE endpoint")
        } else {
            format!(
                "HTTP {status} from SSE endpoint: {}",
                truncate_for_log(body, SSE_ERROR_SNIPPET_LIMIT)
            )
        };
        return SseFailureReport {
            category: SseFailureCategory::HttpStatus,
            status: status_code,
            auth_related: status.as_u16() == 401
                || status.as_u16() == 403
                || is_auth_related_message(body),
            message,
        };
    }

    if let Ok(json) = serde_json::from_str::<serde_json::Value>(body) {
        if let Some(provider_error) = extract_provider_error_details(&json) {
            return SseFailureReport {
                category: SseFailureCategory::ProviderBusinessError,
                status: status_code,
                message: truncate_for_log(
                    &format!(
                        "content-type={content_type}; code={}; message={}",
                        provider_error.code.unwrap_or_default(),
                        provider_error.message
                    ),
                    SSE_ERROR_SNIPPET_LIMIT,
                ),
                auth_related: provider_error.auth_related,
            };
        }
    }

    SseFailureReport {
        category: SseFailureCategory::UnexpectedContentType,
        status: status_code,
        auth_related: is_auth_related_message(body),
        message: truncate_for_log(
            &format!(
                "Expected text/event-stream but got '{content_type}': {}",
                body.trim()
            ),
            SSE_ERROR_SNIPPET_LIMIT,
        ),
    }
}

fn jitter_seed() -> u64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    (now.as_secs() << 32) ^ now.subsec_nanos() as u64
}

fn jittered_duration(base: Duration, jitter_bps: u16, seed: u64) -> Duration {
    if jitter_bps == 0 {
        return base;
    }

    let base_ms = base.as_millis();
    if base_ms == 0 {
        return base;
    }

    let jitter_max = base_ms.saturating_mul(jitter_bps as u128) / 10_000;
    if jitter_max == 0 {
        return base;
    }
    let span = jitter_max.saturating_mul(2).saturating_add(1);
    let offset = (seed as u128 % span) as i128 - jitter_max as i128;
    let jittered_ms = (base_ms as i128 + offset).max(1);
    Duration::from_millis(jittered_ms as u64)
}

#[derive(Debug, Clone)]
struct ProviderErrorDetails {
    code: Option<i64>,
    message: String,
    auth_related: bool,
}

fn extract_provider_error_details(json: &serde_json::Value) -> Option<ProviderErrorDetails> {
    if json.get("jsonrpc").is_some() || json.get("result").is_some() || json.get("error").is_some()
    {
        return None;
    }

    let success_false = json.get("success").and_then(|value| value.as_bool()) == Some(false);
    let code = json.get("code").and_then(|value| value.as_i64());
    let message = json
        .get("msg")
        .or_else(|| json.get("message"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .unwrap_or_default()
        .to_string();
    if !success_false && code.is_none() {
        return None;
    }
    if message.is_empty() {
        return Some(ProviderErrorDetails {
            code,
            message: "upstream provider reported an error".to_string(),
            auth_related: matches!(code, Some(401 | 403 | 1001)),
        });
    }

    Some(ProviderErrorDetails {
        code,
        auth_related: is_auth_related_message(&message) || matches!(code, Some(401 | 403 | 1001)),
        message,
    })
}

fn provider_error_to_mcp_payload(details: &ProviderErrorDetails) -> serde_json::Value {
    let message = if details.auth_related {
        format!(
            "Upstream auth/config error: {}. Ensure --header \"Authorization: Bearer <token>\" or --oauth2-bearer <token> is set.",
            details.message
        )
    } else if let Some(code) = details.code {
        format!("Upstream provider error (code {code}): {}", details.message)
    } else {
        format!("Upstream provider error: {}", details.message)
    };

    serde_json::json!({
        "error": {
            "code": MCP_INTERNAL_ERROR_CODE,
            "message": message,
        }
    })
}

fn is_auth_related_message(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    [
        "auth",
        "token",
        "bearer",
        "unauthorized",
        "forbidden",
        "permission",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn truncate_for_log(message: &str, limit: usize) -> String {
    let normalized = message.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.chars().count() <= limit {
        return normalized;
    }

    let mut result = String::with_capacity(limit + 1);
    for (idx, ch) in normalized.chars().enumerate() {
        if idx >= limit {
            result.push('…');
            break;
        }
        result.push(ch);
    }
    result
}

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
        let mut reconnect_backoff = SseReconnectBackoff::new(
            SSE_RETRY_INITIAL_DELAY,
            SSE_RETRY_MAX_DELAY,
            SSE_RETRY_MULTIPLIER,
            SSE_RETRY_JITTER_BPS,
        );
        let mut log_gate = SseErrorLogGate::default();

        loop {
            let Some(sid) = session_for_sse.read().await.clone() else {
                tokio::time::sleep(SSE_WAIT_FOR_SESSION_DELAY).await;
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
                    log_gate.log_failure(SseFailureReport {
                        category: SseFailureCategory::Connect,
                        status: None,
                        message: truncate_for_log(&err.to_string(), SSE_ERROR_SNIPPET_LIMIT),
                        auth_related: false,
                    });
                    tokio::time::sleep(reconnect_backoff.next_delay()).await;
                    continue;
                }
            };

            let status = response.status();
            let content_type = response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .map(|value| value.to_lowercase())
                .unwrap_or_default();
            if !status.is_success() || !content_type.contains("text/event-stream") {
                let body = response.text().await.unwrap_or_default();
                let report = classify_sse_handshake_failure(status, &content_type, &body);
                log_gate.log_failure(report);
                tokio::time::sleep(reconnect_backoff.next_delay()).await;
                continue;
            }

            reconnect_backoff.reset();
            log_gate.flush_suppressed();
            sse_pool
                .mark_success(&sse_key_clone, "streamable-http")
                .await;

            let stream = response.bytes_stream().eventsource();
            tokio::pin!(stream);
            let mut stream_failed = false;
            while let Some(event) = stream.next().await {
                match event {
                    Ok(event) => {
                        if event.data.trim().is_empty() {
                            continue;
                        }
                        match serde_json::from_str::<serde_json::Value>(&event.data) {
                            Ok(json) => {
                                println!("{}", json);
                            }
                            Err(err) => {
                                log_gate.log_failure(SseFailureReport {
                                    category: SseFailureCategory::NonJsonEvent,
                                    status: None,
                                    message: truncate_for_log(
                                        &format!("{err} (event={})", event.data),
                                        SSE_ERROR_SNIPPET_LIMIT,
                                    ),
                                    auth_related: false,
                                });
                            }
                        }
                    }
                    Err(err) => {
                        log_gate.log_failure(SseFailureReport {
                            category: SseFailureCategory::Stream,
                            status: None,
                            message: truncate_for_log(&err.to_string(), SSE_ERROR_SNIPPET_LIMIT),
                            auth_related: false,
                        });
                        stream_failed = true;
                        break;
                    }
                }
            }
            if !stream_failed {
                log_gate.log_failure(SseFailureReport {
                    category: SseFailureCategory::StreamEnded,
                    status: None,
                    message: "event stream ended".to_string(),
                    auth_related: false,
                });
            }
            tokio::time::sleep(reconnect_backoff.next_delay()).await;
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
        let request_context = StreamableRequestContext {
            http: &http,
            url: &streamable_http_url,
            headers: &runtime_args.headers,
            session_id: &session_clone,
            pool: &pool,
            pool_key: &request_key,
        };
        if !initialized && !is_initialize_request(&message) {
            let init_id = auto_init_id();
            let init_message = create_initialize_request(&init_id, &protocol_version);
            let init_payload = send_request(
                &request_context,
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
            if let Err(err) =
                send_initialized_notification(&request_context, retry_policy, &mut circuit_breaker)
                    .await
            {
                tracing::error!("Failed to send initialized notification: {err}");
            } else {
                initialized = true;
            }
        }

        let payload = send_request(
            &request_context,
            &message,
            retry_policy,
            &mut circuit_breaker,
        )
        .await;

        if is_initialize_request(&message) && payload.get("error").is_none() && !initialized {
            if let Err(err) =
                send_initialized_notification(&request_context, retry_policy, &mut circuit_breaker)
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

struct StreamableRequestContext<'a> {
    http: &'a reqwest::Client,
    url: &'a str,
    headers: &'a HeadersMap,
    session_id: &'a Arc<RwLock<Option<String>>>,
    pool: &'a Arc<TransportPool>,
    pool_key: &'a str,
}

async fn send_request(
    context: &StreamableRequestContext<'_>,
    message: &serde_json::Value,
    retry_policy: RetryPolicy,
    circuit_breaker: &mut CircuitBreaker,
) -> serde_json::Value {
    match run_with_retry(
        "streamable-http-request",
        retry_policy,
        circuit_breaker,
        || async {
            let mut req = apply_request_headers(
                context.http.post(context.url).json(message),
                context.headers,
            );
            if let Some(sid) = context.session_id.read().await.clone() {
                req = req.header("Mcp-Session-Id", sid);
            }
            let resp = req.send().await.map_err(|err| err.to_string())?;
            context
                .pool
                .mark_success(context.pool_key, "streamable-http")
                .await;
            if let Some(sid) = extract_session_id(resp.headers()) {
                *context.session_id.write().await = Some(sid.to_string());
            }
            parse_response_payload(resp).await
        },
    )
    .await
    {
        Ok(payload) => payload,
        Err(err) => error_payload(MCP_INTERNAL_ERROR_CODE, err),
    }
}

async fn send_initialized_notification(
    context: &StreamableRequestContext<'_>,
    retry_policy: RetryPolicy,
    circuit_breaker: &mut CircuitBreaker,
) -> Result<(), String> {
    run_with_retry(
        "streamable-http-initialized-notification",
        retry_policy,
        circuit_breaker,
        || async {
            let message = create_initialized_notification();
            let mut req = apply_request_headers(
                context.http.post(context.url).json(&message),
                context.headers,
            );
            if let Some(sid) = context.session_id.read().await.clone() {
                req = req.header("Mcp-Session-Id", sid);
            }
            let response = req.send().await.map_err(|err| err.to_string())?;
            context
                .pool
                .mark_success(context.pool_key, "streamable-http")
                .await;
            if let Some(sid) = extract_session_id(response.headers()) {
                *context.session_id.write().await = Some(sid.to_string());
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
        if let Some(details) = extract_provider_error_details(&json) {
            return Ok(provider_error_to_mcp_payload(&details));
        }
        return Err(format!("Request failed with status {}", status));
    }
    if let Some(details) = extract_provider_error_details(&json) {
        return Ok(provider_error_to_mcp_payload(&details));
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

    #[test]
    fn provider_business_error_maps_to_mcp_error_payload() {
        let provider = serde_json::json!({
            "code": 1001,
            "msg": "Authentication parameter not received in Header, unable to authenticate",
            "success": false
        });
        let details = extract_provider_error_details(&provider)
            .expect("provider business error should be detected");
        let payload = provider_error_to_mcp_payload(&details);
        assert_eq!(
            payload["error"]["code"],
            serde_json::json!(MCP_INTERNAL_ERROR_CODE)
        );
        let message = payload["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .to_ascii_lowercase();
        assert!(message.contains("auth/config error"));
        assert!(message.contains("authorization: bearer"));
    }

    #[test]
    fn provider_error_detection_ignores_json_rpc_envelopes() {
        let json_rpc = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {"ok": true}
        });
        assert!(extract_provider_error_details(&json_rpc).is_none());
    }

    #[test]
    fn sse_reconnect_backoff_grows_and_resets() {
        let mut backoff = SseReconnectBackoff::new(
            SSE_RETRY_INITIAL_DELAY,
            SSE_RETRY_MAX_DELAY,
            SSE_RETRY_MULTIPLIER,
            SSE_RETRY_JITTER_BPS,
        );

        let first = backoff.next_delay_with_seed(0);
        let second = backoff.next_delay_with_seed(1);

        assert!(first >= Duration::from_millis(400));
        assert!(first <= Duration::from_millis(600));
        assert!(second >= Duration::from_millis(800));
        assert!(second <= Duration::from_millis(1_200));
        assert_eq!(backoff.current_delay, Duration::from_secs(2));

        backoff.reset();
        assert_eq!(backoff.current_delay, SSE_RETRY_INITIAL_DELAY);
    }

    #[test]
    fn sse_log_gate_suppresses_repeats_within_window() {
        let mut gate = SseErrorLogGate::default();
        let now = Instant::now();
        let fingerprint = "connect_error|0|boom";

        let decision1 = gate.decide(fingerprint, now);
        assert!(matches!(decision1, SseLogDecision::Emit { .. }));

        let decision2 = gate.decide(fingerprint, now + Duration::from_secs(1));
        assert!(matches!(decision2, SseLogDecision::Suppress));

        let decision3 = gate.decide(
            fingerprint,
            now + SSE_ERROR_SUPPRESSION_WINDOW + Duration::from_secs(1),
        );
        match decision3 {
            SseLogDecision::Emit { suppressed, .. } => assert_eq!(suppressed, 1),
            SseLogDecision::Suppress => panic!("expected emit after suppression window"),
        }
    }

    #[test]
    fn classify_sse_handshake_detects_provider_auth_error() {
        let body = r#"{"code":1001,"msg":"Authentication parameter not received in Header","success":false}"#;
        let report =
            classify_sse_handshake_failure(reqwest::StatusCode::OK, "application/json", body);
        assert_eq!(report.category, SseFailureCategory::ProviderBusinessError);
        assert!(report.auth_related);
    }
}
