use std::fmt;

#[derive(Debug)]
pub enum ToolCallError {
    InvalidEndpoint(String),
    InvalidArguments(String),
    MissingRequired {
        tool: String,
        path: String,
        key: String,
    },
    ToolNotFound {
        name: String,
    },
    AuthorizationRequired {
        status: u16,
        hint: String,
    },
    Protocol(String),
    Transport(String),
}

impl fmt::Display for ToolCallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidEndpoint(msg) => write!(f, "Invalid endpoint: {msg}"),
            Self::InvalidArguments(msg) => write!(f, "Invalid arguments: {msg}"),
            Self::MissingRequired { tool, path, key } => {
                write!(
                    f,
                    "Missing required argument for tool '{tool}': {path}.{key}"
                )
            }
            Self::ToolNotFound { name } => write!(f, "Tool not found: {name}"),
            Self::AuthorizationRequired { status, hint } => {
                write!(f, "Authorization required (HTTP {status}). {hint}")
            }
            Self::Protocol(msg) => write!(f, "Protocol error: {msg}"),
            Self::Transport(msg) => write!(f, "Transport error: {msg}"),
        }
    }
}

impl std::error::Error for ToolCallError {}
