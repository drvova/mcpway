use std::collections::HashMap;

use serde::Serialize;
use serde_json::{Map, Value};

use crate::tool_api::client::{ToolCatalogMetadata, ToolClient, ToolHandle};
use crate::tool_api::error::ToolCallError;

#[derive(Clone)]
pub struct ErgonomicToolsFacade {
    client: ToolClient,
}

impl ErgonomicToolsFacade {
    pub(crate) fn new(client: ToolClient) -> Self {
        Self { client }
    }

    pub async fn by_name(&self, name: &str) -> Result<ToolHandle, ToolCallError> {
        self.client.tools().by_name(name).await
    }

    pub async fn call(&self, name: &str, args: Value) -> Result<Value, ToolCallError> {
        self.client.call_by_name(name, args).await
    }

    pub async fn call_json(&self, name: &str, args: Value) -> Result<Value, ToolCallError> {
        self.call(name, args).await
    }

    pub async fn call_map(
        &self,
        name: &str,
        args: HashMap<String, Value>,
    ) -> Result<Value, ToolCallError> {
        let object = Map::from_iter(args.into_iter());
        self.call(name, Value::Object(object)).await
    }

    pub async fn call_struct<T: Serialize>(
        &self,
        name: &str,
        args: &T,
    ) -> Result<Value, ToolCallError> {
        let value = serde_json::to_value(args).map_err(|err| {
            ToolCallError::InvalidArguments(format!(
                "Failed to serialize typed arguments for tool '{name}': {err}"
            ))
        })?;
        self.call(name, value).await
    }

    pub async fn prepare_args(&self, name: &str, args: Value) -> Result<Value, ToolCallError> {
        self.client.prepare_args(name, args).await
    }

    pub async fn list(&self) -> Result<Vec<ToolCatalogMetadata>, ToolCallError> {
        self.client.list_with_metadata().await
    }
}
