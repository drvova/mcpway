use std::collections::HashMap;
use std::time::Duration;

use eventsource_stream::Eventsource;
use futures::{SinkExt, StreamExt};
use reqwest::{RequestBuilder, Url};
use serde_json::Value;
use tokio::net::TcpStream;
use tokio::sync::{oneshot, Mutex, RwLock};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};

use crate::tool_api::error::ToolCallError;

const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const SSE_ENDPOINT_TIMEOUT: Duration = Duration::from_secs(10);

type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;
type WsWriter = futures::stream::SplitSink<WsStream, Message>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    StreamableHttp,
    Sse,
    Ws,
}

#[derive(Debug, Clone)]
pub(crate) struct TransportOptions {
    pub(crate) endpoint: String,
    pub(crate) headers: HashMap<String, String>,
    pub(crate) connect_timeout: Duration,
    pub(crate) request_timeout: Option<Duration>,
}

impl Default for TransportOptions {
    fn default() -> Self {
        Self {
            endpoint: String::new(),
            headers: HashMap::new(),
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
            request_timeout: Some(DEFAULT_REQUEST_TIMEOUT),
        }
    }
}

pub(crate) enum TransportClient {
    StreamableHttp(StreamableHttpTransport),
    Sse(SseTransport),
    Ws(WsTransport),
}

impl TransportClient {
    pub(crate) fn new(
        transport: Transport,
        options: TransportOptions,
    ) -> Result<Self, ToolCallError> {
        match transport {
            Transport::StreamableHttp => {
                Ok(Self::StreamableHttp(StreamableHttpTransport::new(options)?))
            }
            Transport::Sse => Ok(Self::Sse(SseTransport::new(options)?)),
            Transport::Ws => Ok(Self::Ws(WsTransport::new(options))),
        }
    }

    pub(crate) async fn send_request(&mut self, request: &Value) -> Result<Value, ToolCallError> {
        match self {
            Self::StreamableHttp(inner) => inner.send_request(request).await,
            Self::Sse(inner) => inner.send_request(request).await,
            Self::Ws(inner) => inner.send_request(request).await,
        }
    }

    pub(crate) async fn send_notification(
        &mut self,
        notification: &Value,
    ) -> Result<(), ToolCallError> {
        match self {
            Self::StreamableHttp(inner) => inner.send_notification(notification).await,
            Self::Sse(inner) => inner.send_notification(notification).await,
            Self::Ws(inner) => inner.send_notification(notification).await,
        }
    }
}

pub(crate) struct StreamableHttpTransport {
    endpoint: String,
    headers: HashMap<String, String>,
    client: reqwest::Client,
    session_id: Option<String>,
}

impl StreamableHttpTransport {
    fn new(options: TransportOptions) -> Result<Self, ToolCallError> {
        Url::parse(&options.endpoint)
            .map_err(|err| ToolCallError::InvalidEndpoint(err.to_string()))?;
        let mut builder = reqwest::Client::builder().connect_timeout(options.connect_timeout);
        if let Some(timeout) = options.request_timeout {
            builder = builder.timeout(timeout);
        }
        let client = builder.build().map_err(|err| {
            ToolCallError::Transport(format!("Failed to build HTTP client: {err}"))
        })?;
        Ok(Self {
            endpoint: options.endpoint,
            headers: options.headers,
            client,
            session_id: None,
        })
    }

    async fn send_request(&mut self, request: &Value) -> Result<Value, ToolCallError> {
        let response = self.dispatch(request).await?;
        response.ok_or_else(|| {
            ToolCallError::Protocol(
                "Expected JSON-RPC response but received empty body".to_string(),
            )
        })
    }

    async fn send_notification(&mut self, notification: &Value) -> Result<(), ToolCallError> {
        let _ = self.dispatch(notification).await?;
        Ok(())
    }

    async fn dispatch(&mut self, payload: &Value) -> Result<Option<Value>, ToolCallError> {
        let mut req = apply_headers(
            self.client.post(&self.endpoint).json(payload),
            &self.headers,
        );
        if let Some(session_id) = &self.session_id {
            req = req.header("Mcp-Session-Id", session_id);
        }
        let response = req
            .send()
            .await
            .map_err(|err| ToolCallError::Transport(err.to_string()))?;
        if let Some(session_id) = extract_session_id(response.headers()) {
            self.session_id = Some(session_id);
        }
        parse_json_response(response, Some(self.endpoint.as_str())).await
    }
}

pub(crate) struct SseTransport {
    sse_endpoint: Url,
    headers: HashMap<String, String>,
    client: reqwest::Client,
    request_timeout: Option<Duration>,
    message_endpoint: std::sync::Arc<RwLock<Option<Url>>>,
    pending: std::sync::Arc<Mutex<HashMap<String, oneshot::Sender<Value>>>>,
    stream_task: Option<JoinHandle<()>>,
}

impl SseTransport {
    fn new(options: TransportOptions) -> Result<Self, ToolCallError> {
        let sse_endpoint = Url::parse(&options.endpoint)
            .map_err(|err| ToolCallError::InvalidEndpoint(err.to_string()))?;
        let client = reqwest::Client::builder()
            .connect_timeout(options.connect_timeout)
            .build()
            .map_err(|err| {
                ToolCallError::Transport(format!("Failed to build SSE client: {err}"))
            })?;

        Ok(Self {
            sse_endpoint,
            headers: options.headers,
            client,
            request_timeout: options.request_timeout,
            message_endpoint: std::sync::Arc::new(RwLock::new(None)),
            pending: std::sync::Arc::new(Mutex::new(HashMap::new())),
            stream_task: None,
        })
    }

    async fn send_request(&mut self, request: &Value) -> Result<Value, ToolCallError> {
        self.ensure_stream_started().await?;
        let message_endpoint = self.wait_for_message_endpoint().await?;
        let request_id = id_key_from_envelope(request).ok_or_else(|| {
            ToolCallError::Protocol("JSON-RPC request is missing an id".to_string())
        })?;

        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(request_id.clone(), tx);

        match self.post_json(message_endpoint, request).await {
            Ok(Some(payload)) => {
                self.pending.lock().await.remove(&request_id);
                return Ok(payload);
            }
            Ok(None) => {}
            Err(err) => {
                self.pending.lock().await.remove(&request_id);
                return Err(err);
            }
        }

        let timeout = self.request_timeout.unwrap_or(DEFAULT_REQUEST_TIMEOUT);
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(payload)) => Ok(payload),
            Ok(Err(_)) => Err(ToolCallError::Transport(
                "SSE stream disconnected while waiting for response".to_string(),
            )),
            Err(_) => {
                self.pending.lock().await.remove(&request_id);
                Err(ToolCallError::Transport(format!(
                    "Timed out waiting for SSE response after {}ms",
                    timeout.as_millis()
                )))
            }
        }
    }

    async fn send_notification(&mut self, notification: &Value) -> Result<(), ToolCallError> {
        self.ensure_stream_started().await?;
        let message_endpoint = self.wait_for_message_endpoint().await?;
        let _ = self.post_json(message_endpoint, notification).await?;
        Ok(())
    }

    async fn ensure_stream_started(&mut self) -> Result<(), ToolCallError> {
        if self
            .stream_task
            .as_ref()
            .map(|task| !task.is_finished())
            .unwrap_or(false)
        {
            return Ok(());
        }

        let mut req = self.client.get(self.sse_endpoint.clone());
        for (k, v) in &self.headers {
            req = req.header(k, v);
        }

        let response = req
            .send()
            .await
            .map_err(|err| ToolCallError::Transport(format!("Failed to open SSE stream: {err}")))?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(map_http_status_error(status, body.trim()));
        }

        *self.message_endpoint.write().await = None;

        let base_url = self.sse_endpoint.clone();
        let pending = self.pending.clone();
        let message_endpoint = self.message_endpoint.clone();

        self.stream_task = Some(tokio::spawn(async move {
            let stream = response.bytes_stream().eventsource();
            tokio::pin!(stream);
            while let Some(event) = stream.next().await {
                let Ok(event) = event else {
                    break;
                };

                if event.event == "endpoint" {
                    if let Ok(endpoint_url) = base_url.join(event.data.trim()) {
                        *message_endpoint.write().await = Some(endpoint_url);
                    }
                    continue;
                }

                if event.data.trim().is_empty() {
                    continue;
                }

                let Ok(payload) = serde_json::from_str::<Value>(&event.data) else {
                    continue;
                };
                let Some(id_key) = id_key_from_envelope(&payload) else {
                    continue;
                };
                if let Some(tx) = pending.lock().await.remove(&id_key) {
                    let _ = tx.send(payload);
                }
            }
        }));

        Ok(())
    }

    async fn wait_for_message_endpoint(&self) -> Result<Url, ToolCallError> {
        let deadline = tokio::time::Instant::now() + SSE_ENDPOINT_TIMEOUT;
        loop {
            if let Some(endpoint) = self.message_endpoint.read().await.clone() {
                return Ok(endpoint);
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(ToolCallError::Transport(format!(
                    "Timed out waiting for SSE message endpoint after {}ms",
                    SSE_ENDPOINT_TIMEOUT.as_millis()
                )));
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    async fn post_json(
        &self,
        endpoint: Url,
        payload: &Value,
    ) -> Result<Option<Value>, ToolCallError> {
        let endpoint_hint = endpoint.to_string();
        let mut req = self.client.post(endpoint).json(payload);
        for (k, v) in &self.headers {
            req = req.header(k, v);
        }
        if let Some(timeout) = self.request_timeout {
            req = req.timeout(timeout);
        }

        let response = req
            .send()
            .await
            .map_err(|err| ToolCallError::Transport(err.to_string()))?;
        parse_json_response(response, Some(endpoint_hint.as_str())).await
    }
}

pub(crate) struct WsTransport {
    endpoint: String,
    headers: HashMap<String, String>,
    connect_timeout: Duration,
    request_timeout: Option<Duration>,
    writer: Option<WsWriter>,
    pending: std::sync::Arc<Mutex<HashMap<String, oneshot::Sender<Value>>>>,
    reader_task: Option<JoinHandle<()>>,
}

impl WsTransport {
    fn new(options: TransportOptions) -> Self {
        Self {
            endpoint: options.endpoint,
            headers: options.headers,
            connect_timeout: options.connect_timeout,
            request_timeout: options.request_timeout,
            writer: None,
            pending: std::sync::Arc::new(Mutex::new(HashMap::new())),
            reader_task: None,
        }
    }

    async fn send_request(&mut self, request: &Value) -> Result<Value, ToolCallError> {
        self.ensure_connected().await?;
        let request_id = id_key_from_envelope(request).ok_or_else(|| {
            ToolCallError::Protocol("JSON-RPC request is missing an id".to_string())
        })?;

        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(request_id.clone(), tx);

        let writer = self.writer.as_mut().ok_or_else(|| {
            ToolCallError::Transport("WebSocket writer is not connected".to_string())
        })?;

        if let Err(err) = writer.send(Message::Text(request.to_string().into())).await {
            self.pending.lock().await.remove(&request_id);
            return Err(ToolCallError::Transport(format!(
                "Failed to send WebSocket message: {err}"
            )));
        }

        let timeout = self.request_timeout.unwrap_or(DEFAULT_REQUEST_TIMEOUT);
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(payload)) => Ok(payload),
            Ok(Err(_)) => Err(ToolCallError::Transport(
                "WebSocket disconnected while waiting for response".to_string(),
            )),
            Err(_) => {
                self.pending.lock().await.remove(&request_id);
                Err(ToolCallError::Transport(format!(
                    "Timed out waiting for WebSocket response after {}ms",
                    timeout.as_millis()
                )))
            }
        }
    }

    async fn send_notification(&mut self, notification: &Value) -> Result<(), ToolCallError> {
        self.ensure_connected().await?;
        let writer = self.writer.as_mut().ok_or_else(|| {
            ToolCallError::Transport("WebSocket writer is not connected".to_string())
        })?;
        writer
            .send(Message::Text(notification.to_string().into()))
            .await
            .map_err(|err| {
                ToolCallError::Transport(format!("Failed to send WebSocket message: {err}"))
            })
    }

    async fn ensure_connected(&mut self) -> Result<(), ToolCallError> {
        if self
            .reader_task
            .as_ref()
            .map(|task| task.is_finished())
            .unwrap_or(false)
        {
            self.writer = None;
            self.reader_task = None;
        }

        if self.writer.is_some() {
            return Ok(());
        }

        let mut request = self
            .endpoint
            .as_str()
            .into_client_request()
            .map_err(|err| ToolCallError::InvalidEndpoint(err.to_string()))?;

        for (key, value) in &self.headers {
            let header_name = tokio_tungstenite::tungstenite::http::header::HeaderName::from_bytes(
                key.as_bytes(),
            )
            .map_err(|err| {
                ToolCallError::InvalidArguments(format!(
                    "Invalid WebSocket header name '{key}': {err}"
                ))
            })?;
            let header_value =
                tokio_tungstenite::tungstenite::http::header::HeaderValue::from_str(value)
                    .map_err(|err| {
                        ToolCallError::InvalidArguments(format!(
                            "Invalid WebSocket header value for '{key}': {err}"
                        ))
                    })?;
            request.headers_mut().insert(header_name, header_value);
        }

        let (stream, _) = tokio::time::timeout(self.connect_timeout, connect_async(request))
            .await
            .map_err(|_| {
                ToolCallError::Transport(format!(
                    "Timed out connecting to WebSocket endpoint after {}ms",
                    self.connect_timeout.as_millis()
                ))
            })?
            .map_err(|err| {
                ToolCallError::Transport(format!("WebSocket connection failed: {err}"))
            })?;

        let (writer, mut reader) = stream.split();
        let pending = self.pending.clone();

        self.reader_task = Some(tokio::spawn(async move {
            while let Some(frame) = reader.next().await {
                let Ok(frame) = frame else {
                    break;
                };

                let payload = match frame {
                    Message::Text(text) => serde_json::from_str::<Value>(&text).ok(),
                    Message::Binary(bytes) => serde_json::from_slice::<Value>(&bytes).ok(),
                    Message::Close(_) => break,
                    Message::Ping(_) | Message::Pong(_) => None,
                    _ => None,
                };

                let Some(payload) = payload else {
                    continue;
                };

                let Some(id_key) = id_key_from_envelope(&payload) else {
                    continue;
                };

                if let Some(tx) = pending.lock().await.remove(&id_key) {
                    let _ = tx.send(payload);
                }
            }
        }));

        self.writer = Some(writer);
        Ok(())
    }
}

pub(crate) fn id_key_from_envelope(message: &Value) -> Option<String> {
    let id = message.get("id")?;
    Some(match id {
        Value::String(s) => format!("s:{s}"),
        Value::Number(n) => format!("n:{n}"),
        Value::Bool(v) => format!("b:{v}"),
        Value::Null => "null".to_string(),
        other => format!("j:{}", other),
    })
}

fn apply_headers(mut req: RequestBuilder, headers: &HashMap<String, String>) -> RequestBuilder {
    for (k, v) in headers {
        req = req.header(k, v);
    }
    req
}

fn extract_session_id(headers: &reqwest::header::HeaderMap) -> Option<String> {
    headers
        .get("Mcp-Session-Id")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
}

async fn parse_json_response(
    response: reqwest::Response,
    _endpoint_hint: Option<&str>,
) -> Result<Option<Value>, ToolCallError> {
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(map_http_status_error(status, body.trim()));
    }

    let bytes = response
        .bytes()
        .await
        .map_err(|err| ToolCallError::Transport(err.to_string()))?;

    if bytes.is_empty() {
        return Ok(None);
    }

    let payload = serde_json::from_slice::<Value>(&bytes)
        .map_err(|err| ToolCallError::Protocol(format!("Response was not JSON: {err}")))?;
    Ok(Some(payload))
}

fn map_http_status_error(status: reqwest::StatusCode, body: &str) -> ToolCallError {
    ToolCallError::Transport(format!("HTTP {} {}", status, body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unauthorized_status_maps_to_transport_error() {
        let err = map_http_status_error(reqwest::StatusCode::UNAUTHORIZED, "unauthorized");

        match err {
            ToolCallError::Transport(message) => assert!(message.contains("401")),
            other => panic!("unexpected error variant: {other}"),
        }
    }
}
