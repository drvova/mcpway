use std::collections::HashMap;
use std::time::Duration;

use crate::tool_api::client::ToolClient;
use crate::tool_api::error::ToolCallError;
use crate::tool_api::transport::{Transport, TransportClient, TransportOptions};

const DEFAULT_PROTOCOL_VERSION: &str = "2024-11-05";
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

pub struct ToolClientBuilder {
    endpoint: String,
    transport: Transport,
    protocol_version: String,
    headers: HashMap<String, String>,
    connect_timeout: Duration,
    request_timeout: Option<Duration>,
}

impl ToolClientBuilder {
    pub fn new(endpoint: impl Into<String>, transport: Transport) -> Self {
        Self {
            endpoint: endpoint.into(),
            transport,
            protocol_version: DEFAULT_PROTOCOL_VERSION.to_string(),
            headers: HashMap::new(),
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
            request_timeout: Some(DEFAULT_REQUEST_TIMEOUT),
        }
    }

    pub fn protocol_version(mut self, protocol_version: impl Into<String>) -> Self {
        self.protocol_version = protocol_version.into();
        self
    }

    pub fn header(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.insert(key.into(), value.into());
        self
    }

    pub fn headers(mut self, headers: HashMap<String, String>) -> Self {
        self.headers.extend(headers);
        self
    }

    pub fn connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = timeout;
        self
    }

    pub fn request_timeout(mut self, timeout: Option<Duration>) -> Self {
        self.request_timeout = timeout;
        self
    }

    pub fn build(self) -> Result<ToolClient, ToolCallError> {
        if self.endpoint.trim().is_empty() {
            return Err(ToolCallError::InvalidEndpoint(
                "endpoint cannot be empty".to_string(),
            ));
        }

        let transport = TransportClient::new(
            self.transport,
            TransportOptions {
                endpoint: self.endpoint,
                headers: self.headers,
                connect_timeout: self.connect_timeout,
                request_timeout: self.request_timeout,
            },
        )?;

        Ok(ToolClient::from_parts(transport, self.protocol_version))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_rejects_empty_endpoint() {
        let result = ToolClientBuilder::new("  ", Transport::StreamableHttp).build();
        assert!(matches!(result, Err(ToolCallError::InvalidEndpoint(_))));
    }
}
