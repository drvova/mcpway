pub mod admin;
pub mod prompt;
pub mod store;

use crate::runtime::store::RuntimeArgsUpdate;
use serde::Serialize;
use tokio::sync::oneshot;

#[derive(Debug, Clone)]
pub enum RuntimeScope {
    Global,
    Session(String),
}

#[derive(Debug, Clone)]
pub struct RuntimeUpdate {
    pub scope: RuntimeScope,
    pub update: RuntimeArgsUpdate,
}

#[derive(Debug, Serialize)]
pub struct RuntimeApplyResult {
    pub status: String,
    pub message: String,
    pub restart: bool,
}

impl RuntimeApplyResult {
    pub fn ok(message: impl Into<String>, restart: bool) -> Self {
        Self {
            status: "ok".to_string(),
            message: message.into(),
            restart,
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self {
            status: "error".to_string(),
            message: message.into(),
            restart: false,
        }
    }
}

pub struct RuntimeUpdateRequest {
    pub update: RuntimeUpdate,
    pub respond_to: oneshot::Sender<RuntimeApplyResult>,
}
