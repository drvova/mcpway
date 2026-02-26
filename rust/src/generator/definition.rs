use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct ParsedServerDefinition {
    pub server_name: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub headers: BTreeMap<String, String>,
}

pub fn load_definition(
    definition_path: &Path,
    requested_server: Option<&str>,
) -> Result<ParsedServerDefinition, String> {
    let contents = fs::read_to_string(definition_path).map_err(|err| {
        format!(
            "Failed to read definition {}: {err}",
            definition_path.display()
        )
    })?;
    let root: Value = serde_json::from_str(&contents)
        .map_err(|err| format!("Invalid JSON in {}: {err}", definition_path.display()))?;
    parse_definition_value(&root, requested_server)
}

fn parse_definition_value(
    root: &Value,
    requested_server: Option<&str>,
) -> Result<ParsedServerDefinition, String> {
    let Some(root_obj) = root.as_object() else {
        return Err("Definition JSON must be an object".to_string());
    };

    let Some(servers_val) = root_obj.get("mcpServers") else {
        return Err("Definition must contain top-level 'mcpServers' object".to_string());
    };
    let Some(servers_obj) = servers_val.as_object() else {
        return Err("mcpServers must be an object".to_string());
    };
    if servers_obj.is_empty() {
        return Err("mcpServers is empty".to_string());
    }

    let (server_name, server_value) = if let Some(requested) = requested_server {
        let Some(server_value) = servers_obj.get(requested) else {
            return Err(format!("Server key '{requested}' not found in mcpServers"));
        };
        (requested.to_string(), server_value)
    } else if servers_obj.len() == 1 {
        let (name, value) = servers_obj
            .iter()
            .next()
            .ok_or_else(|| "mcpServers is empty".to_string())?;
        (name.to_string(), value)
    } else {
        return Err("Definition has multiple mcpServers entries; pass --server <name>".to_string());
    };

    parse_server_object(server_name, server_value)
}

fn parse_server_object(
    server_name: String,
    server_value: &Value,
) -> Result<ParsedServerDefinition, String> {
    let Some(server_obj) = server_value.as_object() else {
        return Err(format!(
            "Server definition for '{server_name}' must be an object"
        ));
    };

    let command = server_obj
        .get("command")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .ok_or_else(|| format!("Server '{server_name}' is missing a non-empty 'command'"))?
        .to_string();

    let args = parse_string_array(server_obj.get("args"), "args", &server_name)?;
    let env = parse_string_map(server_obj.get("env"), "env", &server_name)?;
    let headers = parse_string_map(server_obj.get("headers"), "headers", &server_name)?;

    Ok(ParsedServerDefinition {
        server_name,
        command,
        args,
        env,
        headers,
    })
}

fn parse_string_array(
    value: Option<&Value>,
    field: &str,
    server_name: &str,
) -> Result<Vec<String>, String> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let Some(arr) = value.as_array() else {
        return Err(format!(
            "Server '{server_name}' field '{field}' must be an array"
        ));
    };

    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        let Some(item_str) = item.as_str() else {
            return Err(format!(
                "Server '{server_name}' field '{field}' must contain only strings"
            ));
        };
        out.push(item_str.to_string());
    }
    Ok(out)
}

fn parse_string_map(
    value: Option<&Value>,
    field: &str,
    server_name: &str,
) -> Result<BTreeMap<String, String>, String> {
    let Some(value) = value else {
        return Ok(BTreeMap::new());
    };
    let Some(obj) = value.as_object() else {
        return Err(format!(
            "Server '{server_name}' field '{field}' must be an object"
        ));
    };

    let mut out = BTreeMap::new();
    for (key, val) in obj {
        let Some(val_str) = val.as_str() else {
            return Err(format!(
                "Server '{server_name}' field '{field}.{key}' must be a string"
            ));
        };
        out.insert(key.to_string(), val_str.to_string());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_root_server_object_without_mcp_servers() {
        let root: Value = serde_json::json!({
            "command": "node",
            "args": ["server.js"],
            "env": {"API_KEY": "secret"},
            "headers": {"Authorization": "Bearer abc"}
        });

        let err = parse_definition_value(&root, None).expect_err("parse should fail");
        assert!(err.contains("mcpServers"));
    }

    #[test]
    fn parses_single_server_from_mcp_servers() {
        let root: Value = serde_json::json!({
            "mcpServers": {
                "default": {
                    "command": "node",
                    "args": ["server.js"],
                    "env": {"API_KEY": "secret"},
                    "headers": {"Authorization": "Bearer abc"}
                }
            }
        });

        let parsed = parse_definition_value(&root, None).expect("parse should succeed");
        assert_eq!(parsed.server_name, "default");
        assert_eq!(parsed.command, "node");
        assert_eq!(parsed.args, vec!["server.js"]);
        assert_eq!(parsed.env.get("API_KEY"), Some(&"secret".to_string()));
        assert_eq!(
            parsed.headers.get("Authorization"),
            Some(&"Bearer abc".to_string())
        );
    }

    #[test]
    fn parses_mcp_servers_map_with_selection() {
        let root: Value = serde_json::json!({
            "mcpServers": {
                "alpha": { "command": "python", "args": ["-m", "alpha"] },
                "beta": { "command": "python", "args": ["-m", "beta"] }
            }
        });

        let parsed = parse_definition_value(&root, Some("beta")).expect("parse should succeed");
        assert_eq!(parsed.server_name, "beta");
        assert_eq!(parsed.command, "python");
        assert_eq!(parsed.args, vec!["-m", "beta"]);
    }

    #[test]
    fn requires_server_name_when_map_has_multiple_entries() {
        let root: Value = serde_json::json!({
            "mcpServers": {
                "alpha": { "command": "python" },
                "beta": { "command": "node" }
            }
        });

        let err = parse_definition_value(&root, None).expect_err("parse should fail");
        assert!(err.contains("multiple mcpServers"));
    }
}
