use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::tool_api::ergonomic::ErgonomicToolsFacade;
use crate::tool_api::error::ToolCallError;
use crate::tool_api::schema::{apply_defaults, validate_required};
use crate::tool_api::transport::TransportClient;

#[derive(Debug, Clone)]
pub struct ToolMetadata {
    pub name: String,
    pub description: Option<String>,
    pub input_schema: Value,
}

#[derive(Debug, Clone)]
pub struct ToolCatalogMetadata {
    pub name: String,
    pub description: Option<String>,
    pub required_keys: usize,
    pub defaulted_keys: usize,
}

#[derive(Clone)]
pub struct ToolClient {
    state: Arc<Mutex<ClientState>>,
}

pub struct ToolsFacade {
    client: ToolClient,
}

#[derive(Clone)]
pub struct ToolHandle {
    client: ToolClient,
    metadata: ToolMetadata,
}

pub(crate) struct ClientState {
    transport: TransportClient,
    protocol_version: String,
    request_seq: u64,
    initialized: bool,
    tools: Vec<ToolMetadata>,
    tools_by_name: HashMap<String, ToolMetadata>,
}

impl ToolClient {
    pub(crate) fn from_parts(transport: TransportClient, protocol_version: String) -> Self {
        Self {
            state: Arc::new(Mutex::new(ClientState {
                transport,
                protocol_version,
                request_seq: 0,
                initialized: false,
                tools: Vec::new(),
                tools_by_name: HashMap::new(),
            })),
        }
    }

    pub fn tools(&self) -> ToolsFacade {
        ToolsFacade {
            client: self.clone(),
        }
    }

    pub fn ergonomic(&self) -> ErgonomicToolsFacade {
        ErgonomicToolsFacade::new(self.clone())
    }

    pub async fn refresh_tools(&self) -> Result<(), ToolCallError> {
        let mut state = self.state.lock().await;
        state.ensure_initialized().await?;
        let response = state.send_jsonrpc_request("tools/list", json!({})).await?;
        let tools = parse_tools_list_response(&response)?;
        state.replace_tools(tools);
        Ok(())
    }

    async fn tool_by_name(&self, name: &str) -> Result<ToolMetadata, ToolCallError> {
        let state = self.state.lock().await;
        state.resolve_tool(name)
    }

    async fn resolve_tool_with_refresh(&self, name: &str) -> Result<ToolMetadata, ToolCallError> {
        match self.tool_by_name(name).await {
            Ok(metadata) => Ok(metadata),
            Err(ToolCallError::ToolNotFound { .. }) => {
                self.refresh_tools().await?;
                self.tool_by_name(name).await
            }
            Err(err) => Err(err),
        }
    }

    async fn list_tools(&self) -> Vec<ToolMetadata> {
        let state = self.state.lock().await;
        state.tools.clone()
    }

    pub async fn list_with_metadata(&self) -> Result<Vec<ToolCatalogMetadata>, ToolCallError> {
        self.refresh_tools().await?;
        let state = self.state.lock().await;
        Ok(state.list_with_metadata())
    }

    pub async fn prepare_args(&self, name: &str, args: Value) -> Result<Value, ToolCallError> {
        let metadata = self.resolve_tool_with_refresh(name).await?;
        normalize_args_for_tool(&metadata, args)
    }

    pub async fn call_by_name(&self, name: &str, args: Value) -> Result<Value, ToolCallError> {
        let metadata = self.resolve_tool_with_refresh(name).await?;
        self.call_tool(&metadata, args).await
    }

    async fn call_tool(&self, metadata: &ToolMetadata, args: Value) -> Result<Value, ToolCallError> {
        let args_object = normalize_args_for_tool(metadata, args)?;

        let mut state = self.state.lock().await;
        state.ensure_initialized().await?;
        state
            .send_jsonrpc_request(
                "tools/call",
                json!({
                    "name": metadata.name,
                    "arguments": args_object,
                }),
            )
            .await
    }
}

impl ToolsFacade {
    pub async fn by_name(&self, name: &str) -> Result<ToolHandle, ToolCallError> {
        let metadata = self.client.resolve_tool_with_refresh(name).await?;
        Ok(ToolHandle {
            client: self.client.clone(),
            metadata,
        })
    }

    pub async fn list(&self) -> Vec<ToolMetadata> {
        self.client.list_tools().await
    }
}

impl ToolHandle {
    pub fn metadata(&self) -> &ToolMetadata {
        &self.metadata
    }

    pub async fn call(&self, args: Value) -> Result<Value, ToolCallError> {
        self.client.call_tool(&self.metadata, args).await
    }
}

impl ClientState {
    async fn ensure_initialized(&mut self) -> Result<(), ToolCallError> {
        if self.initialized {
            return Ok(());
        }

        let request_id = self.next_request_id();
        let request = json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "method": "initialize",
            "params": {
                "protocolVersion": self.protocol_version,
                "capabilities": {
                    "roots": { "listChanged": true },
                    "sampling": {}
                },
                "clientInfo": {
                    "name": "mcpway-tool-api",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }
        });

        let response = self.transport.send_request(&request).await?;
        ensure_no_rpc_error("initialize", &response)?;

        let initialized_notification = json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        });
        self.transport
            .send_notification(&initialized_notification)
            .await?;

        self.initialized = true;
        Ok(())
    }

    async fn send_jsonrpc_request(&mut self, method: &str, params: Value) -> Result<Value, ToolCallError> {
        let request = json!({
            "jsonrpc": "2.0",
            "id": self.next_request_id(),
            "method": method,
            "params": params,
        });

        let response = self.transport.send_request(&request).await?;
        ensure_no_rpc_error(method, &response)?;
        Ok(response)
    }

    fn resolve_tool(&self, name: &str) -> Result<ToolMetadata, ToolCallError> {
        self.tools_by_name
            .get(name)
            .cloned()
            .ok_or_else(|| ToolCallError::ToolNotFound {
                name: name.to_string(),
            })
    }

    fn replace_tools(&mut self, tools: Vec<ToolMetadata>) {
        let mut tools_by_name = HashMap::new();
        for tool in &tools {
            tools_by_name.insert(tool.name.clone(), tool.clone());
        }

        self.tools = tools;
        self.tools_by_name = tools_by_name;
    }

    fn list_with_metadata(&self) -> Vec<ToolCatalogMetadata> {
        self.tools
            .iter()
            .map(|tool| ToolCatalogMetadata {
                name: tool.name.clone(),
                description: tool.description.clone(),
                required_keys: count_required_keys(&tool.input_schema),
                defaulted_keys: count_defaulted_keys(&tool.input_schema),
            })
            .collect()
    }

    fn next_request_id(&mut self) -> String {
        self.request_seq = self.request_seq.saturating_add(1);
        format!("tool-api-{}", self.request_seq)
    }
}

fn normalize_args_for_tool(metadata: &ToolMetadata, args: Value) -> Result<Value, ToolCallError> {
    let mut args_object = args;
    if !args_object.is_object() {
        return Err(ToolCallError::InvalidArguments(format!(
            "Tool '{}' expects JSON object arguments",
            metadata.name
        )));
    }

    apply_defaults(&metadata.input_schema, &mut args_object);
    validate_required(&metadata.name, &metadata.input_schema, &args_object)?;
    Ok(args_object)
}

fn parse_tools_list_response(response: &Value) -> Result<Vec<ToolMetadata>, ToolCallError> {
    let result = response
        .get("result")
        .ok_or_else(|| ToolCallError::Protocol("tools/list response missing result".to_string()))?;
    let tools = result
        .get("tools")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            ToolCallError::Protocol("tools/list result missing tools array".to_string())
        })?;

    let mut parsed = Vec::with_capacity(tools.len());
    for tool in tools {
        let Some(obj) = tool.as_object() else {
            return Err(ToolCallError::Protocol(
                "tools/list item was not an object".to_string(),
            ));
        };

        let name = obj
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                ToolCallError::Protocol("tool entry missing non-empty name".to_string())
            })?
            .to_string();

        let description = obj
            .get("description")
            .and_then(Value::as_str)
            .map(str::to_string);

        let input_schema = obj
            .get("inputSchema")
            .cloned()
            .unwrap_or_else(|| json!({"type": "object"}));

        parsed.push(ToolMetadata {
            name,
            description,
            input_schema,
        });
    }

    Ok(parsed)
}

fn ensure_no_rpc_error(method: &str, response: &Value) -> Result<(), ToolCallError> {
    if response.get("error").is_none() {
        return Ok(());
    }

    Err(ToolCallError::Protocol(format!(
        "RPC method '{method}' returned error: {}",
        response.get("error").unwrap_or(&Value::Null)
    )))
}

fn count_required_keys(schema: &Value) -> usize {
    let mut total = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|values| values.iter().filter(|value| value.is_string()).count())
        .unwrap_or(0);

    let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
        return total;
    };

    for property in properties.values() {
        total += count_required_keys(property);
    }

    total
}

fn count_defaulted_keys(schema: &Value) -> usize {
    let mut total = 0usize;
    let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
        return total;
    };

    for property in properties.values() {
        if property.get("default").is_some() {
            total = total.saturating_add(1);
        }
        total = total.saturating_add(count_defaulted_keys(property));
    }

    total
}

pub use crate::tool_api::transport::Transport;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_counts_include_nested_required_and_defaults() {
        let schema = json!({
            "type": "object",
            "required": ["city"],
            "properties": {
                "city": {"type": "string"},
                "units": {"type": "string", "default": "metric"},
                "filters": {
                    "type": "object",
                    "required": ["region"],
                    "properties": {
                        "region": {"type": "string"},
                        "lang": {"type": "string", "default": "en"}
                    }
                }
            }
        });

        assert_eq!(count_required_keys(&schema), 2);
        assert_eq!(count_defaulted_keys(&schema), 2);
    }

    #[test]
    fn list_with_metadata_reports_required_and_default_counts() {
        let mut state = ClientState {
            transport: TransportClient::new(
                Transport::StreamableHttp,
                crate::tool_api::transport::TransportOptions {
                    endpoint: "http://127.0.0.1:1".to_string(),
                    headers: HashMap::new(),
                    connect_timeout: std::time::Duration::from_secs(1),
                    request_timeout: Some(std::time::Duration::from_secs(1)),
                },
            )
            .expect("transport init"),
            protocol_version: "2024-11-05".to_string(),
            request_seq: 0,
            initialized: false,
            tools: Vec::new(),
            tools_by_name: HashMap::new(),
        };

        state.replace_tools(vec![ToolMetadata {
            name: "weather-report".to_string(),
            description: Some("Weather lookup".to_string()),
            input_schema: json!({
                "type": "object",
                "required": ["city"],
                "properties": {
                    "city": {"type": "string"},
                    "units": {"type": "string", "default": "metric"}
                }
            }),
        }]);

        let list = state.list_with_metadata();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "weather-report");
        assert_eq!(list[0].required_keys, 1);
        assert_eq!(list[0].defaulted_keys, 1);
    }

    #[test]
    fn id_key_from_envelope_handles_numeric_and_string_ids() {
        assert_eq!(
            crate::tool_api::transport::id_key_from_envelope(&json!({"id": "abc"})),
            Some("s:abc".to_string())
        );
        assert_eq!(
            crate::tool_api::transport::id_key_from_envelope(&json!({"id": 42})),
            Some("n:42".to_string())
        );
    }
}
