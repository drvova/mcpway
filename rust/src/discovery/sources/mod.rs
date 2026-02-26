pub mod claude;
pub mod codex;
pub mod cursor;
pub mod nodecode;
pub mod opencode;
pub mod vscode;
pub mod windsurf;

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::Value;

use crate::discovery::{DiscoveredTransport, DiscoveryIssue, DiscoveryIssueLevel, SourceKind};

pub fn infer_transport(
    command: Option<&str>,
    url: Option<&str>,
    explicit_type: Option<&str>,
) -> Option<DiscoveredTransport> {
    if command.map(|v| !v.trim().is_empty()).unwrap_or(false) {
        return Some(DiscoveredTransport::Stdio);
    }

    let typ = explicit_type
        .map(|v| v.trim().to_ascii_lowercase())
        .unwrap_or_default();

    if !typ.is_empty() {
        if typ.contains("ws") {
            return Some(DiscoveredTransport::Ws);
        }
        if typ.contains("sse") {
            return Some(DiscoveredTransport::Sse);
        }
        if typ.contains("http") || typ.contains("streamable") {
            return Some(DiscoveredTransport::StreamableHttp);
        }
    }

    if let Some(url) = url {
        if let Ok(parsed) = url::Url::parse(url) {
            match parsed.scheme() {
                "ws" | "wss" => return Some(DiscoveredTransport::Ws),
                "http" | "https" => {
                    let has_sse_segment = parsed
                        .path_segments()
                        .map(|mut segments| {
                            segments.any(|segment| segment.eq_ignore_ascii_case("sse"))
                        })
                        .unwrap_or(false);
                    if has_sse_segment {
                        return Some(DiscoveredTransport::Sse);
                    }
                    return Some(DiscoveredTransport::StreamableHttp);
                }
                _ => {}
            }
        }
    }

    None
}

pub fn json_string_map(value: Option<&Value>) -> BTreeMap<String, String> {
    let Some(obj) = value.and_then(Value::as_object) else {
        return BTreeMap::new();
    };

    let mut out = BTreeMap::new();
    for (key, value) in obj {
        if let Some(as_str) = value.as_str() {
            out.insert(key.to_string(), as_str.to_string());
        }
    }
    out
}

pub fn json_string_array(value: Option<&Value>) -> Vec<String> {
    let Some(arr) = value.and_then(Value::as_array) else {
        return Vec::new();
    };

    arr.iter()
        .filter_map(Value::as_str)
        .map(ToString::to_string)
        .collect()
}

pub fn load_json(path: &Path) -> Result<Value, String> {
    let body = std::fs::read_to_string(path)
        .map_err(|err| format!("Failed to read {}: {err}", path.display()))?;
    serde_json::from_str(&body).map_err(|err| format!("Invalid JSON in {}: {err}", path.display()))
}

pub fn load_json_or_jsonc(path: &Path) -> Result<Value, String> {
    let body = std::fs::read_to_string(path)
        .map_err(|err| format!("Failed to read {}: {err}", path.display()))?;

    match serde_json::from_str::<Value>(&body) {
        Ok(value) => Ok(value),
        Err(_) => {
            let stripped = strip_jsonc_comments(&body);
            serde_json::from_str(&stripped)
                .map_err(|err| format!("Invalid JSON/JSONC in {}: {err}", path.display()))
        }
    }
}

pub fn load_toml(path: &Path) -> Result<toml::Value, String> {
    let body = std::fs::read_to_string(path)
        .map_err(|err| format!("Failed to read {}: {err}", path.display()))?;
    toml::from_str(&body).map_err(|err| format!("Invalid TOML in {}: {err}", path.display()))
}

pub fn strip_jsonc_comments(input: &str) -> String {
    let mut out = String::new();
    let mut in_string = false;
    let mut escaped = false;
    let mut chars = input.chars().peekable();

    while let Some(ch) = chars.next() {
        if in_string {
            out.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }

        if ch == '"' {
            in_string = true;
            out.push(ch);
            continue;
        }

        if ch == '/' {
            match chars.peek() {
                Some('/') => {
                    let _ = chars.next();
                    for next in chars.by_ref() {
                        if next == '\n' {
                            out.push('\n');
                            break;
                        }
                    }
                    continue;
                }
                Some('*') => {
                    let _ = chars.next();
                    let mut prev = '\0';
                    for next in chars.by_ref() {
                        if prev == '*' && next == '/' {
                            break;
                        }
                        prev = next;
                    }
                    continue;
                }
                _ => {}
            }
        }

        out.push(ch);
    }

    out
}

pub fn source_issue(source: SourceKind, path: &Path, message: impl Into<String>) -> DiscoveryIssue {
    DiscoveryIssue {
        level: DiscoveryIssueLevel::Warning,
        source,
        origin_path: path.to_string_lossy().to_string(),
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn infer_transport_detects_sse_from_path_segment() {
        let transport = infer_transport(None, Some("https://example.com/sse"), None);
        assert_eq!(transport, Some(DiscoveredTransport::Sse));
    }

    #[test]
    fn infer_transport_ignores_transport_query_hint() {
        let transport = infer_transport(None, Some("https://example.com/mcp?transport=sse"), None);
        assert_eq!(transport, Some(DiscoveredTransport::StreamableHttp));
    }
}
