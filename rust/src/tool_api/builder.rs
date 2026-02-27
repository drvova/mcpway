use std::collections::HashMap;
use std::time::Duration;

use crate::tool_api::client::ToolClient;
use crate::tool_api::error::ToolCallError;
use crate::tool_api::transport::{Transport, TransportClient, TransportOptions};

const DEFAULT_PROTOCOL_VERSION: &str = "2024-11-05";
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_millis(1_500);
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

pub struct ToolClientBuilder {
    endpoint: String,
    transport: Transport,
    protocol_version: String,
    headers: HashMap<String, String>,
    connect_timeout: Duration,
    request_timeout: Option<Duration>,
    stdio_command: Option<String>,
    stdio_args: Vec<String>,
    stdio_env: HashMap<String, String>,
    stdio_cwd: Option<String>,
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
            stdio_command: None,
            stdio_args: Vec::new(),
            stdio_env: HashMap::new(),
            stdio_cwd: None,
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

    pub fn stdio_command(mut self, command: impl Into<String>) -> Self {
        self.stdio_command = Some(command.into());
        self
    }

    pub fn stdio_args(mut self, args: Vec<String>) -> Self {
        self.stdio_args = args;
        self
    }

    pub fn stdio_env(mut self, env: HashMap<String, String>) -> Self {
        self.stdio_env = env;
        self
    }

    pub fn stdio_cwd(mut self, cwd: Option<String>) -> Self {
        self.stdio_cwd = cwd;
        self
    }

    pub fn build(self) -> Result<ToolClient, ToolCallError> {
        match self.transport {
            Transport::Stdio => {
                let command = self
                    .stdio_command
                    .as_deref()
                    .map(str::trim)
                    .unwrap_or_default();
                if command.is_empty() {
                    return Err(ToolCallError::InvalidArguments(
                        "stdio command cannot be empty".to_string(),
                    ));
                }
            }
            Transport::StreamableHttp | Transport::Sse | Transport::Ws | Transport::Grpc => {
                if self.endpoint.trim().is_empty() {
                    return Err(ToolCallError::InvalidEndpoint(
                        "endpoint cannot be empty".to_string(),
                    ));
                }
            }
        }

        let transport = TransportClient::new(
            self.transport,
            TransportOptions {
                endpoint: self.endpoint,
                headers: self.headers,
                connect_timeout: self.connect_timeout,
                request_timeout: self.request_timeout,
                stdio_command: self.stdio_command,
                stdio_args: self.stdio_args,
                stdio_env: self.stdio_env,
                stdio_cwd: self.stdio_cwd,
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

    #[test]
    fn build_rejects_stdio_without_command() {
        let result = ToolClientBuilder::new("", Transport::Stdio).build();
        assert!(matches!(result, Err(ToolCallError::InvalidArguments(_))));
    }

    #[test]
    fn build_accepts_stdio_with_command() {
        let result = ToolClientBuilder::new("", Transport::Stdio)
            .stdio_command("node")
            .stdio_args(vec!["--version".to_string()])
            .build();
        assert!(result.is_ok());
    }
}
